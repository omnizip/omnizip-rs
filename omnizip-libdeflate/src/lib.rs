//! omnizip-libdeflate — pure-Rust libdeflate-compatible DEFLATE codec.
//!
//! **Status: skeleton (TODO 104 Phase 1).** The crate exists so the
//! `CodecId::LIBDEFLATE = 0x000B` slot is occupied and the codec
//! registry can dispatch to it. The current implementation delegates
//! to `miniz_oxide` (the same backend `omnizip-deflate` uses) — this
//! is functionally correct but provides no speed advantage yet.
//!
//! ## Roadmap (TODO 104)
//!
//! - **Phase 2 — Decode pipeline** (8 days): in-house RFC 1951
//!   inflate with a fast 4096-entry Huffman table and refill-heavy
//!   bit reader. Target: 1.5× `omnizip-deflate` decode throughput.
//! - **Phase 3 — Encode pipeline** (3 days, optional): canonical
//!   Huffman + simple LZ77. Target: ratio within 5% of `zlib -6`.
//!
//! ## Wire format
//!
//! Standard RFC 1951 DEFLATE. The codec is byte-compatible with
//! `gzip -d`, `zlib.decompress`, and any other DEFLATE decoder. The
//! "libdeflate-compatible" label refers to the implementation
//! strategy (faster Huffman, refill-heavy bit reader), not a new
//! wire format.
//!
//! ## Why a separate crate?
//!
//! `omnizip-deflate` wraps `miniz_oxide` and exposes its own
//! `DeflateCodec`. `omnizip-libdeflate` exists as a separate crate
//! because:
//!
//! 1. **Different codec id.** `DeflateCodec = 0x0005`,
//!    `LibdeflateCodec = 0x000B`. LimniFS uses the id to route
//!    decode traffic to the fastest available implementation.
//! 2. **Different optimisation target.** `miniz_oxide` prioritises
//!    small binary size; libdeflate prioritises decode speed. The
//!    two have different trade-offs and shouldn't share an
//!    implementation.
//! 3. **OCP.** Adding a new codec should be a new crate + one
//!    `register()` call, not edits to an existing crate.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// Libdeflate-compatible DEFLATE codec.
///
/// Currently delegates to `miniz_oxide` (Phase 1 skeleton). See the
/// [crate-level docs](self) for the roadmap.
#[derive(Clone, Copy, Debug, Default)]
pub struct LibdeflateCodec;

impl LibdeflateCodec {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Codec for LibdeflateCodec {
    fn id(&self) -> CodecId {
        CodecId::LIBDEFLATE
    }

    fn name(&self) -> &'static str {
        "libdeflate"
    }

    fn compress(
        &self,
        plaintext: &[u8],
        level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        // Phase 1 skeleton: delegate to miniz_oxide with zlib wrapping.
        // Phase 3 will replace this with an in-house encoder.
        let _ = level;
        let deflate = miniz_oxide::deflate::compress_to_vec_zlib(plaintext, 6);
        Ok(deflate)
    }

    fn decompress(
        &self,
        compressed: &[u8],
        expected_len: u32,
    ) -> Result<Vec<u8>, OmnizipError> {
        // Phase 1 skeleton: delegate to miniz_oxide. auto-detect
        // zlib/gzip/raw via wbits=0 (which means "auto" in our wrapper).
        // Phase 2 will replace this with an in-house decoder.
        let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::LIBDEFLATE,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;
        // Try zlib first (most common), then raw.
        let decoded = miniz_oxide::inflate::decompress_to_vec_zlib(compressed)
            .or_else(|_| miniz_oxide::inflate::decompress_to_vec(compressed))
            .map_err(|e| OmnizipError::DecodeFailed {
                codec: CodecId::LIBDEFLATE,
                reason: format!("inflate failed: {e}"),
            })?;
        if decoded.len() != expected_us {
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::LIBDEFLATE,
                expected: expected_len,
                actual: decoded.len(),
            });
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_basic() {
        let input = b"the quick brown fox jumps over the lazy dog ".to_vec();
        let compressed = LibdeflateCodec
            .compress(&input, CompressionLevel::default())
            .expect("compress");
        let decompressed = LibdeflateCodec
            .decompress(&compressed, input.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn codec_id_is_reserved_slot() {
        assert_eq!(LibdeflateCodec.id(), CodecId::LIBDEFLATE);
        assert_eq!(LibdeflateCodec.name(), "libdeflate");
    }

    #[test]
    fn round_trip_empty() {
        let compressed = LibdeflateCodec
            .compress(b"", CompressionLevel::default())
            .expect("compress empty");
        let decompressed = LibdeflateCodec
            .decompress(&compressed, 0)
            .expect("decompress empty");
        assert!(decompressed.is_empty());
    }
}
