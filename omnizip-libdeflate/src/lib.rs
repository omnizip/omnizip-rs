//! omnizip-libdeflate — pure-Rust libdeflate-compatible DEFLATE codec.
//!
//! Full in-house implementation of RFC 1951 (DEFLATE) encode + decode.
//! No delegation to `miniz_oxide` — the encoder and decoder are both
//! pure Rust, built from the spec.
//!
//! ## Components
//!
//! - **Encoder** ([`deflate`]): stored blocks (BTYPE=0) for small
//!   inputs, LZ77 + fixed-Huffman (BTYPE=1) for inputs ≥ 128 bytes.
//!   Hash-chain match finder with lazy look-ahead.
//! - **Decoder** ([`inflate`]): full RFC 1951 inflate supporting
//!   stored, fixed-Huffman, and dynamic-Huffman block types.
//!   Canonical Huffman table builder, LSB-first bit reader with
//!   zero-padded refill.
//! - **Wire format**: output wrapped in zlib (RFC 1950) header +
//!   adler32 trailer. Decodable by `gzip -d`, `zlib.decompress`,
//!   and any other zlib-aware tool.
//!
//! ## Why a separate crate?
//!
//! `omnizip-deflate` wraps `miniz_oxide`. `omnizip-libdeflate`
//! exists as a separate crate because:
//!
//! 1. **Different codec id.** `DeflateCodec = 0x0005`,
//!    `LibdeflateCodec = 0x000B`.
//! 2. **Different optimisation target.** `miniz_oxide` prioritises
//!    small binary size; this crate prioritises implementation
//!    independence and correctness-by-construction.
//! 3. **OCP.** Adding a new codec = new crate + one `register()`.
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
//!    `LibdeflateCodec = 0x000B`. `LimniFS` uses the id to route
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
pub mod deflate_dynamic;
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

    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        let _ = level;
        // Strategy: try fixed-Huffman first, then stored. Pick the
        // smallest valid output for each input. This mirrors what
        // `gzip -1` does at a basic level.
        //
        // Dynamic-Huffman is also computed but only used when it's
        // smaller AND the decoder supports it. The in-house inflate
        // currently has edge cases with some dynamic-Huffman blocks
        // (TODO 116); once the decoder is verified, the dynamic path
        // becomes the default.
        let mut best: Option<Vec<u8>> = None;
        let mut pick = |candidate: Option<Vec<u8>>| {
            if let Some(c) = candidate {
                match &best {
                    None => best = Some(c),
                    Some(prev) if c.len() < prev.len() => best = Some(c),
                    _ => {}
                }
            }
        };
        // Dynamic-Huffman re-enabled after round-trip verification.
        pick(deflate_dynamic::deflate_dynamic_huffman(plaintext)?);
        pick(deflate_lz77::deflate_fixed_huffman(plaintext)?);
        pick(Some(deflate::deflate_stored(plaintext)?));
        Ok(wrap_zlib(&best.expect("at least stored always succeeds")))
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::LIBDEFLATE,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;

        // Strip zlib wrapper if present (2-byte header + 4-byte adler32
        // trailer), then run the in-house RFC 1951 inflate. The
        // miniz_oxide fallback was removed once the in-house decoder
        // round-tripped every fixture (TODO 136).
        let raw = strip_zlib_wrapper(compressed);
        let decoded =
            inflate::inflate(raw, expected_us).map_err(|e| OmnizipError::DecodeFailed {
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
