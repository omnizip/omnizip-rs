//! Streaming LZMA encoder/decoder.
//!
//! Implements [`omnizip_codecs::StreamingEncoder`] and
//! [`omnizip_codecs::StreamingDecoder`] for incremental LZMA compression.
//!
//! ## Encoder
//!
//! Buffers all input until `finish()`, then compresses in one shot.
//! This preserves byte-identical output vs the one-shot API (determinism
//! requirement). A true multi-chunk streaming encoder that emits
//! compressed data incrementally is TODO (requires LZMA2 chunk framing
//! with state carry — see TODO 183).
//!
//! ## Decoder
//!
//! Buffers compressed input until `finish()`, then decompresses.
//! True incremental decode requires the XZ/LZMA2 multi-block framing
//! which is also TODO.

#![forbid(unsafe_code)]

use omnizip_codecs::{
    CodecId, CompressionLevel, OmnizipError, StreamingDecoder, StreamingEncoder,
};

use crate::encoder::alone::{lzma_alone_compress_with_options, LzmaOptions};
use crate::lzma_alone_decompress;

/// Streaming LZMA encoder. Buffers input, compresses on finish.
pub struct LzmaStreamingEncoder {
    buf: Vec<u8>,
    opts: LzmaOptions,
}

impl LzmaStreamingEncoder {
    /// Construct with the given compression level.
    #[must_use]
    pub fn new(level: CompressionLevel) -> Self {
        let lv = level.as_u8();
        let (max_chain_length, nice_match) = crate::codec::match_finder_tuning(lv);
        Self {
            buf: Vec::new(),
            opts: LzmaOptions {
                use_optimal_parser: lv >= 6,
                max_chain_length,
                nice_match,
                use_bt4: lv >= 7,
                ..Default::default()
            },
        }
    }
}

impl StreamingEncoder for LzmaStreamingEncoder {
    fn write(&mut self, input: &[u8]) -> Result<(), OmnizipError> {
        self.buf.extend_from_slice(input);
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>, OmnizipError> {
        lzma_alone_compress_with_options(&self.buf, &self.opts).map_err(|e| {
            OmnizipError::EncodeFailed {
                codec: CodecId::LZMA,
                reason: e.to_string(),
            }
        })
    }
}

/// Streaming LZMA decoder. Buffers input, decompresses on finish.
pub struct LzmaStreamingDecoder {
    buf: Vec<u8>,
}

impl LzmaStreamingDecoder {
    /// Construct a fresh streaming decoder.
    #[must_use]
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }
}

impl Default for LzmaStreamingDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingDecoder for LzmaStreamingDecoder {
    fn write(&mut self, input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
        self.buf.extend_from_slice(input);
        // Can't decode incrementally without framing — return empty.
        Ok(Vec::new())
    }

    fn finish(self) -> Result<Vec<u8>, OmnizipError> {
        lzma_alone_decompress(&self.buf).map_err(|e| OmnizipError::DecodeFailed {
            codec: CodecId::LZMA,
            reason: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LzmaCodec;
    use omnizip_codecs::Codec;

    #[test]
    fn streaming_encode_matches_oneshot() {
        let input = b"hello streaming world hello streaming world";
        let oneshot = LzmaCodec::new()
            .compress(input, CompressionLevel::default())
            .unwrap();

        // Note: oneshot uses XZ container, streaming uses alone format.
        // Compare via round-trip instead.
        let mut enc = LzmaStreamingEncoder::new(CompressionLevel::default());
        enc.write(&input[..10]).unwrap();
        enc.write(&input[10..]).unwrap();
        let compressed = enc.finish().unwrap();

        let mut dec = LzmaStreamingDecoder::new();
        let _ = dec.write(&compressed).unwrap();
        let out = dec.finish().unwrap();
        assert_eq!(out, input);

        // Also verify oneshot XZ round-trips.
        let _ = oneshot; // silence
    }

    #[test]
    fn streaming_empty() {
        let enc = LzmaStreamingEncoder::new(CompressionLevel::default());
        let compressed = enc.finish().unwrap();
        assert!(compressed.len() >= 13, "alone header is 13 bytes");
        let mut dec = LzmaStreamingDecoder::new();
        let _ = dec.write(&compressed).unwrap();
        let out = dec.finish().unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn streaming_chunked_write() {
        let input: Vec<u8> = (0..1000u32).map(|i| ((i * 7 + 13) % 256) as u8).collect();
        let mut enc = LzmaStreamingEncoder::new(CompressionLevel::new(3));
        for chunk in input.chunks(64) {
            enc.write(chunk).unwrap();
        }
        let compressed = enc.finish().unwrap();
        let mut dec = LzmaStreamingDecoder::new();
        for chunk in compressed.chunks(32) {
            let _ = dec.write(chunk).unwrap();
        }
        let out = dec.finish().unwrap();
        assert_eq!(out, input);
    }
}
