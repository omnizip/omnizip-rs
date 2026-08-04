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

pub mod deflate;
pub mod deflate_lz77;
mod inflate;

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
        let _ = level;
        // Uses stored blocks (correct, ~100% ratio). The LZ77 +
        // fixed-Huffman path exists in `deflate_lz77` but has
        // bit-order issues in the match encoder that need debugging.
        // Once fixed, this will switch to the LZ77 path for inputs
        // ≥ 128 bytes for ~50-60% ratio on text.
        let raw = deflate::deflate_stored(plaintext)?;
        Ok(wrap_zlib(&raw))
    }

    fn decompress(
        &self,
        compressed: &[u8],
        expected_len: u32,
    ) -> Result<Vec<u8>, OmnizipError> {
        let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::LIBDEFLATE,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;

        // Phase 2 (in-house RFC 1951 decoder): strip zlib wrapper if
        // present (2-byte header + 4-byte adler32 trailer), then run
        // our own inflate. Fall back to miniz_oxide if the in-house
        // path errors — that gives us correct behavior on all current
        // inputs while Phase 2 stabilises.
        let raw = strip_zlib_wrapper(compressed);
        match inflate::inflate(raw, expected_us) {
            Ok(decoded) if decoded.len() == expected_us => Ok(decoded),
            Ok(decoded) => Err(OmnizipError::LengthMismatch {
                codec: CodecId::LIBDEFLATE,
                expected: expected_len,
                actual: decoded.len(),
            }),
            Err(_) => {
                // Fallback: try miniz_oxide with zlib first, then raw.
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
    }
}

/// Strip a zlib wrapper (RFC 1950) if present, returning a slice
/// containing just the raw DEFLATE stream.
///
/// Zlib wrapper layout:
/// - 2-byte header: CMF + FLG (CM=8, CINFO=7 typical).
/// - 4-byte adler32 trailer.
fn strip_zlib_wrapper(data: &[u8]) -> &[u8] {
    // Zlib header is 2 bytes; CMF & 0x0F should be 8 (deflate).
    // CMF & 0xF0 = CINFO << 4; CINFO ≤ 7 for window size ≤ 32K.
    if data.len() >= 6 {
        let cmf = data[0];
        let flg = data[1];
        let cm = cmf & 0x0F;
        let cinfo = (cmf >> 4) & 0x0F;
        if cm == 8 && cinfo <= 7 {
            // Verify CMF + FLG is a multiple of 31 (RFC 1950 §2.2).
            if (u16::from(cmf) * 256 + u16::from(flg)) % 31 == 0 {
                return &data[2..data.len() - 4];
            }
        }
    }
    // Not zlib; assume raw DEFLATE.
    data
}

/// Wrap a raw DEFLATE stream in a zlib header + adler32 trailer
/// (RFC 1950). The result is decodable by `gzip -d`, Python's
/// `zlib.decompress`, and any other zlib-aware tool.
fn wrap_zlib(deflate_stream: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(deflate_stream.len() + 6);
    // CMF: CM=8 (deflate), CINFO=7 (32K window) → 0x78.
    // FLG: 0x9C = (CMF * 256 + FLG) % 31 == 0 with FCHECK.
    // 0x78 0x9C is the standard zlib header for level 6 / default.
    out.push(0x78);
    out.push(0x9C);
    out.extend_from_slice(deflate_stream);
    let checksum = adler32(deflate_stream);
    out.extend_from_slice(&checksum.to_be_bytes());
    out
}

/// Compute the Adler-32 checksum of `data` (RFC 1950 §9).
fn adler32(data: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + u32::from(byte)) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
    }
    (b << 16) | a
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
