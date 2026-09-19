//! Streaming encode/decode traits for incremental compression.
//!
//! Extends the one-shot [`Codec`] trait with incremental APIs for
//! processing data that doesn't fit in memory. OCP: existing `Codec`
//! implementations are unchanged; codecs opt into streaming by
//! implementing these traits.
//!
//! ## Design
//!
//! ```ignore
//! let mut enc = LzmaStreamingEncoder::new(level);
//! enc.write(chunk1)?;
//! enc.write(chunk2)?;
//! let compressed = enc.finish()?;
//!
//! let mut dec = LzmaStreamingDecoder::new();
//! let partial = dec.write(&compressed[..100])?;
//! let rest = dec.write(&compressed[100..])?;
//! let final_bytes = dec.finish()?;
//! ```
//!
//! ## Determinism
//!
//! Streaming encode MUST produce byte-identical output to the one-shot
//! `compress` for the same input + level (when all data is written
//! before `finish`). This is a hard requirement for `LimniFS`.

#![forbid(unsafe_code)]

use crate::OmnizipError;

/// Incremental encoder. Write data in chunks, then call [`finish`](Self::finish)
/// to get the complete compressed output.
pub trait StreamingEncoder {
    /// Write a chunk of plaintext. May buffer internally.
    ///
    /// # Errors
    ///
    /// Returns an error on encode failure.
    fn write(&mut self, input: &[u8]) -> Result<(), OmnizipError>;

    /// Finish encoding and return the complete compressed output.
    ///
    /// # Errors
    ///
    /// Returns an error on encode failure or if no data was written.
    fn finish(self) -> Result<Vec<u8>, OmnizipError>;
}

/// Incremental decoder. Write compressed data in chunks; each call may
/// return zero or more decoded plaintext bytes.
pub trait StreamingDecoder {
    /// Write a chunk of compressed data. Returns any plaintext that
    /// could be fully decoded from the data received so far.
    ///
    /// # Errors
    ///
    /// Returns an error on decode failure or corruption.
    fn write(&mut self, input: &[u8]) -> Result<Vec<u8>, OmnizipError>;

    /// Finish decoding and return any remaining plaintext.
    ///
    /// # Errors
    ///
    /// Returns an error if the stream is truncated or corrupt.
    fn finish(self) -> Result<Vec<u8>, OmnizipError>;
}

// ============================================================================
// ChunkedStreamEncoder — bounded-memory streaming over ANY codec
// (TODO.ref-parity/57, encoder leg).
//
// The determinism rule: the output is a pure function of (input
// bytes, input length, declared chunk_size) — the implementation
// buffers internally until a full chunk is assembled, so WHEN bytes
// arrive via `write` never affects the output. Each chunk is
// encoded as an independent stream and the results concatenate in
// chunk order; the concatenation contract is the same one
// parallel_compress documents (zstd multi-frame, gzip multi-member,
// bzip2/xz multistream).
// ============================================================================

/// [`StreamingEncoder`] over any [`Codec`](crate::Codec), with a
/// declared chunk size that owns the output contract.
///
/// Memory bound: `chunk_size` of buffered plaintext plus the
/// in-flight chunk's compressed form.
pub struct ChunkedStreamEncoder {
    codec: Box<dyn crate::Codec>,
    level: crate::CompressionLevel,
    chunk_size: usize,
    buf: Vec<u8>,
    out: Vec<u8>,
}

impl ChunkedStreamEncoder {
    /// Stream-compress with `codec` at `level`, one independent
    /// stream per `chunk_size` plaintext bytes. Owns the codec, so
    /// call sites stay lifetime-free.
    #[must_use]
    pub fn new(
        codec: Box<dyn crate::Codec>,
        level: crate::CompressionLevel,
        chunk_size: usize,
    ) -> Self {
        Self {
            codec,
            level,
            chunk_size: chunk_size.max(1),
            buf: Vec::new(),
            out: Vec::new(),
        }
    }

    fn flush_chunk(&mut self) -> Result<(), OmnizipError> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::take(&mut self.buf);
        let compressed = self.codec.compress(&chunk, self.level)?;
        self.out.extend_from_slice(&compressed);
        Ok(())
    }
}

impl StreamingEncoder for ChunkedStreamEncoder {
    fn write(&mut self, input: &[u8]) -> Result<(), OmnizipError> {
        self.buf.extend_from_slice(input);
        while self.buf.len() >= self.chunk_size {
            // Split at the declared boundary: buf[drain] is the chunk.
            let chunk: Vec<u8> = self.buf.drain(..self.chunk_size).collect();
            let compressed = self.codec.compress(&chunk, self.level)?;
            self.out.extend_from_slice(&compressed);
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<u8>, OmnizipError> {
        self.flush_chunk()?;
        if self.out.is_empty() {
            // No input at all: an empty chunk still encodes (codecs'
            // empty-input behavior is part of their contract).
            return self.codec.compress(&[], self.level);
        }
        Ok(self.out)
    }
}

/// [`StreamingDecoder`] over any [`Codec`](crate::Codec) — the
/// decoder leg of task 57.
///
/// ## v1 semantics (documented, not hidden)
///
/// Compressed bytes buffer until [`finish`](StreamingDecoder::finish)
/// — push partitioning never changes the result, and the decode
/// runs once over the concatenation. Output memory is bounded per
/// call (each `write` returns what was decodable so far: nothing,
/// in v1); the INPUT buffer is the caller's compressed stream,
/// typically many times smaller than the plaintext it expands to.
/// Incremental per-frame decoding (zstd magic-scanning) is the
/// follow-up recorded in the task file.
pub struct ChunkedStreamDecoder {
    codec: Box<dyn crate::Codec>,
    expected_len: u32,
    buf: Vec<u8>,
}

impl ChunkedStreamDecoder {
    /// `expected_len` = the exact plaintext size when known;
    /// `u32::MAX` delegates to codecs' length-agnostic paths where
    /// they exist (the same contract the FFI's unknown-length
    /// decode uses).
    #[must_use]
    pub fn new(codec: Box<dyn crate::Codec>, expected_len: u32) -> Self {
        Self {
            codec,
            expected_len,
            buf: Vec::new(),
        }
    }
}

impl StreamingDecoder for ChunkedStreamDecoder {
    fn write(&mut self, input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
        self.buf.extend_from_slice(input);
        // v1: decode happens at finish; nothing decodable to return
        // incrementally yet.
        Ok(Vec::new())
    }

    fn finish(mut self) -> Result<Vec<u8>, OmnizipError> {
        let compressed = std::mem::take(&mut self.buf);
        self.codec.decompress(&compressed, self.expected_len)
    }
}

#[cfg(test)]
mod chunked_tests {
    use super::{ChunkedStreamEncoder, StreamingEncoder};
    use crate::codec::CodecId;
    use crate::level::CompressionLevel;
    use crate::{Codec, OmnizipError};

    /// A codec whose output = 4-byte LE length tag + payload, so
    /// chunk boundaries are observable in the output.
    struct TaggedCodec;

    impl Codec for TaggedCodec {
        fn id(&self) -> CodecId {
            CodecId::new(0xFF03)
        }
        fn name(&self) -> &'static str {
            "tagged"
        }
        fn compress(&self, p: &[u8], _l: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
            let mut out = u32::try_from(p.len()).unwrap().to_le_bytes().to_vec();
            out.extend_from_slice(p);
            Ok(out)
        }
        fn decompress(&self, _c: &[u8], _e: u32) -> Result<Vec<u8>, OmnizipError> {
            unimplemented!("test codec")
        }
    }

    fn data(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| u8::try_from(i % 249).expect("<249"))
            .collect()
    }

    fn push_all(
        enc: &mut ChunkedStreamEncoder,
        input: &[u8],
        partition: &[usize],
    ) -> Result<(), OmnizipError> {
        let mut i = 0;
        for &n in partition {
            enc.write(&input[i..i + n])?;
            i += n;
        }
        assert_eq!(i, input.len());
        Ok(())
    }

    /// THE determinism property: the same input + chunk size produce
    /// byte-identical output regardless of how writes were
    /// partitioned (1B..chunk_size random boundaries, seeded).
    #[test]
    fn partition_invariance() -> Result<(), OmnizipError> {
        let mut rng = 0xC0FFEE_u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for (len, chunk) in [(0_usize, 16), (1, 16), (100, 16), (4096, 512), (5000, 1024)] {
            let input = data(len);
            let level = CompressionLevel::default();
            let baseline = {
                let mut e = ChunkedStreamEncoder::new(Box::new(TaggedCodec), level, chunk);
                e.write(&input)?;
                e.finish()?
            };
            for trial in 0..8 {
                let mut partition = Vec::new();
                let mut left = len;
                while left > 0 {
                    let n = (next() as usize % chunk + 1).min(left);
                    partition.push(n);
                    left -= n;
                }
                let mut e = ChunkedStreamEncoder::new(Box::new(TaggedCodec), level, chunk);
                push_all(&mut e, &input, &partition)?;
                assert_eq!(
                    e.finish()?,
                    baseline,
                    "len {len} chunk {chunk} trial {trial} partition {partition:?}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn chunk_boundaries_in_output() {
        // 3000 bytes, chunk 1024 -> 3 chunks: tags 1024, 1024, 952.
        let input = data(3000);
        let mut e =
            ChunkedStreamEncoder::new(Box::new(TaggedCodec), CompressionLevel::default(), 1024);
        e.write(&input).unwrap();
        let out = e.finish().unwrap();
        let mut expect = Vec::new();
        for c in input.chunks(1024) {
            expect.extend_from_slice(&u32::try_from(c.len()).unwrap().to_le_bytes());
            expect.extend_from_slice(c);
        }
        assert_eq!(out, expect);
    }

    #[test]
    fn single_chunk_equals_one_shot() {
        let input = data(777);
        let level = CompressionLevel::default();
        let mut e = ChunkedStreamEncoder::new(Box::new(TaggedCodec), level, 4096);
        e.write(&input).unwrap();
        assert_eq!(
            e.finish().unwrap(),
            TaggedCodec.compress(&input, level).unwrap()
        );
    }

    #[test]
    fn empty_input_encodes_empty_chunk() {
        let level = CompressionLevel::default();
        let mut e = ChunkedStreamEncoder::new(Box::new(TaggedCodec), level, 64);
        let out = e.finish().unwrap();
        assert_eq!(out, TaggedCodec.compress(&[], level).unwrap());
    }
}

#[cfg(test)]
mod chunked_decoder_tests {
    use super::{ChunkedStreamDecoder, StreamingDecoder};
    use crate::codec::CodecId;
    use crate::level::CompressionLevel;
    use crate::{Codec, OmnizipError};

    struct UpperCodec;

    impl Codec for UpperCodec {
        fn id(&self) -> CodecId {
            CodecId::new(0xFF04)
        }
        fn name(&self) -> &'static str {
            "upper"
        }
        fn compress(&self, p: &[u8], _l: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
            Ok(p.iter().map(|b| b.to_ascii_uppercase()).collect())
        }
        fn decompress(&self, c: &[u8], expected: u32) -> Result<Vec<u8>, OmnizipError> {
            let out: Vec<u8> = c.iter().map(|b| b.to_ascii_lowercase()).collect();
            if out.len() as u32 != expected && expected != u32::MAX {
                return Err(OmnizipError::LengthMismatch {
                    codec: self.id(),
                    expected,
                    actual: out.len(),
                });
            }
            Ok(out)
        }
    }

    /// Partition invariance: the same compressed stream decodes to
    /// the same plaintext regardless of write partitioning.
    #[test]
    fn partition_invariance() -> Result<(), OmnizipError> {
        let compressed =
            UpperCodec.compress(b"Hello Streaming World", CompressionLevel::default())?;
        let mut partitions: Vec<Vec<usize>> = vec![vec![compressed.len()]];
        let mut i = 0;
        while i < compressed.len() {
            partitions.push(vec![i + 1, compressed.len() - i - 1]);
            i += 7;
        }
        for partition in &partitions {
            let mut d = ChunkedStreamDecoder::new(Box::new(UpperCodec), 21);
            let mut offset = 0;
            for &n in partition {
                d.write(&compressed[offset..offset + n])?;
                offset += n;
            }
            assert_eq!(offset, compressed.len());
            assert_eq!(
                d.finish()?,
                b"hello streaming world",
                "partition {partition:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn unknown_length_and_errors() -> Result<(), OmnizipError> {
        let compressed = UpperCodec.compress(b"ABC", CompressionLevel::default())?;
        let mut d = ChunkedStreamDecoder::new(Box::new(UpperCodec), u32::MAX);
        d.write(&compressed)?;
        assert_eq!(d.finish()?, b"abc");

        let mut d = ChunkedStreamDecoder::new(Box::new(UpperCodec), 99);
        d.write(&compressed)?;
        assert!(d.finish().is_err());
        Ok(())
    }
}
