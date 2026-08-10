//! Streaming LZ4 compression.
//!
//! Implements [`StreamingEncoder`] / [`StreamingDecoder`] for LZ4.
//! LZ4's chunk-based format makes streaming natural: each call to
//! `write` that produces >= 64 KiB of input emits one block.
//!
//! ## Determinism
//!
//! Same input (in any chunk size) → byte-identical output. We buffer
//! internally to a 64 KiB block boundary, so chunk boundaries in the
//! input don't change the output.

use omnizip_codecs::level::CompressionLevel;
use omnizip_codecs::{OmnizipError, StreamingDecoder, StreamingEncoder};

const BLOCK_SIZE: usize = 64 * 1024;

/// Streaming LZ4 compressor. Write plaintext chunks via
/// [`write`](StreamingEncoder::write); call
/// [`finish`](StreamingEncoder::finish) to flush and get the complete
/// compressed output.
pub struct Lz4StreamingEncoder {
    level: CompressionLevel,
    pending: Vec<u8>,
    output: Vec<u8>,
}

impl Lz4StreamingEncoder {
    /// Create a new LZ4 streamer at the given compression level.
    #[must_use]
    pub fn new(level: CompressionLevel) -> Self {
        Self {
            level,
            pending: Vec::new(),
            output: Vec::new(),
        }
    }

    fn flush_full_blocks(&mut self) {
        while self.pending.len() >= BLOCK_SIZE {
            let block: Vec<u8> = self.pending.drain(..BLOCK_SIZE).collect();
            let compressed = if self.level.as_u8() >= 4 {
                crate::hc::compress(&block)
            } else {
                crate::block::compress_block(&block)
            };
            // Frame per block: 4-byte LE compressed length + 4-byte LE
            // original length + compressed bytes. Including the original
            // length lets the decompressor allocate the right buffer
            // for the last (smaller) block.
            self.output
                .extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            self.output
                .extend_from_slice(&(block.len() as u32).to_le_bytes());
            self.output.extend_from_slice(&compressed);
        }
    }
}

impl StreamingEncoder for Lz4StreamingEncoder {
    fn write(&mut self, input: &[u8]) -> Result<(), OmnizipError> {
        self.pending.extend_from_slice(input);
        self.flush_full_blocks();
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<u8>, OmnizipError> {
        // Final block: flush remaining input as one (possibly small) block.
        if !self.pending.is_empty() {
            let original_len = self.pending.len();
            let compressed = if self.level.as_u8() >= 4 {
                crate::hc::compress(&self.pending)
            } else {
                crate::block::compress_block(&self.pending)
            };
            self.output
                .extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            self.output
                .extend_from_slice(&(original_len as u32).to_le_bytes());
            self.output.extend_from_slice(&compressed);
            self.pending.clear();
        }

        // Append two zero u32s as end-of-stream marker.
        self.output.extend_from_slice(&0u32.to_le_bytes());
        self.output.extend_from_slice(&0u32.to_le_bytes());
        Ok(std::mem::take(&mut self.output))
    }
}

/// Streaming LZ4 decompressor.
pub struct Lz4StreamingDecoder {
    pending: Vec<u8>,
    output: Vec<u8>,
    finished: bool,
}

impl Lz4StreamingDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            output: Vec::new(),
            finished: false,
        }
    }
}

impl Default for Lz4StreamingDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingDecoder for Lz4StreamingDecoder {
    fn write(&mut self, input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.pending.extend_from_slice(input);

        // Each block frame: 4-byte LE compressed length + 4-byte LE
        // original length + compressed bytes.
        while self.pending.len() >= 8 {
            let compressed_len = u32::from_le_bytes([
                self.pending[0],
                self.pending[1],
                self.pending[2],
                self.pending[3],
            ]) as usize;
            let original_len = u32::from_le_bytes([
                self.pending[4],
                self.pending[5],
                self.pending[6],
                self.pending[7],
            ]) as usize;
            if compressed_len == 0 && original_len == 0 {
                // End-of-stream marker.
                self.pending.drain(..8);
                self.finished = true;
                break;
            }
            if self.pending.len() < 8 + compressed_len {
                break; // need more input
            }
            let block_bytes: Vec<u8> = self.pending.drain(..8 + compressed_len).skip(8).collect();
            let decompressed =
                crate::block::decompress_block(&block_bytes, original_len).map_err(|e| {
                    OmnizipError::decode_failed(
                        omnizip_codecs::CodecId::LZ4,
                        format!("LZ4 block decode failed: {e}"),
                    )
                })?;
            self.output.extend_from_slice(&decompressed);
        }

        Ok(std::mem::take(&mut self.output))
    }

    fn finish(self) -> Result<Vec<u8>, OmnizipError> {
        Ok(self.output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stream_round_trips() {
        let mut enc = Lz4StreamingEncoder::new(CompressionLevel::new(1));
        enc.write(&[]).expect("write");
        let compressed = enc.finish().expect("finish");
        assert_eq!(compressed.len(), 8); // just end-of-stream marker

        let mut dec = Lz4StreamingDecoder::new();
        let out = dec.write(&compressed).expect("write");
        assert!(out.is_empty());
    }

    #[test]
    fn small_stream_round_trips() {
        let input = b"hello world hello world hello world";

        let mut enc = Lz4StreamingEncoder::new(CompressionLevel::new(1));
        enc.write(input).expect("write");
        let compressed = enc.finish().expect("finish");

        let mut dec = Lz4StreamingDecoder::new();
        let out = dec.write(&compressed).expect("write");
        assert_eq!(out.as_slice(), &input[..]);
    }

    #[test]
    fn chunked_input_matches_single_shot() {
        let input: Vec<u8> = (0..200_000).map(|i| (i & 0xFF) as u8).collect();

        // Single write
        let mut enc1 = Lz4StreamingEncoder::new(CompressionLevel::new(1));
        enc1.write(&input).expect("write");
        let out1 = enc1.finish().expect("finish");

        // Chunked: 1000-byte chunks
        let mut enc2 = Lz4StreamingEncoder::new(CompressionLevel::new(1));
        for chunk in input.chunks(1000) {
            enc2.write(chunk).expect("write");
        }
        let out2 = enc2.finish().expect("finish");

        // Same input → same output (determinism).
        assert_eq!(out1, out2);
    }

    #[test]
    fn large_input_multi_block() {
        // 200 KB input → multiple 64 KB blocks.
        let input: Vec<u8> = (0..200_000).map(|i| (i & 0xFF) as u8).collect();

        let mut enc = Lz4StreamingEncoder::new(CompressionLevel::new(1));
        enc.write(&input).expect("write");
        let compressed = enc.finish().expect("finish");

        let mut dec = Lz4StreamingDecoder::new();
        let out = dec.write(&compressed).expect("write");
        assert_eq!(out.len(), input.len());
        assert_eq!(out, input);
    }
}
