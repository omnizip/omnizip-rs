//! omnizip-glza — Pure-Rust GLZA (Grammar-based LZ) compression codec.
//!
//! GLZA builds a context-free grammar over the input: repeated substrings
//! are promoted to non-terminal rules, and the compressed form is the
//! grammar itself (start rule + rule definitions).
//!
//! ## Algorithm
//!
//! 1. Build a suffix array of the input.
//! 2. Walk the LCP array to find the most frequent repeated substring
//!    (length >= 4, occurrences >= 2).
//! 3. Promote that substring to a non-terminal rule and replace every
//!    non-overlapping occurrence with a rule reference.
//! 4. Repeat until no candidate improves compression.
//! 5. Serialize the grammar with a simple varint-based encoding.
//!
//! Phase 1 limitations:
//! - Suffix sort is O(n (log n)^2) prefix-doubling (not SA-IS).
//! - Greedy extraction (one rule per pass, full re-sort each pass).
//! - No entropy coding on the rule bodies (symbols stored raw).
//!
//! ## Determinism
//!
//! The output is byte-identical for identical inputs across runs: the
//! suffix array, LCP array, greedy extraction order, and serialization are
//! all deterministic and tied only to the input bytes.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

mod decode;
mod encode;
mod grammar;
mod suffix_array;

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

pub use decode::GLZA_CODEC_ID;
pub use grammar::{Grammar, Symbol};

/// Compress `input` with GLZA.
///
/// # Errors
///
/// Returns [`OmnizipError::EncodeFailed`] only on internal errors (currently
/// never — the encoder is total).
pub fn compress(input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    let grammar = Grammar::build(input);
    let uncompressed_size = u32::try_from(input.len()).map_err(|_| OmnizipError::EncodeFailed {
        codec: GLZA_CODEC_ID,
        reason: format!("input length {} exceeds u32::MAX", input.len()),
    })?;
    Ok(encode::encode(&grammar, uncompressed_size))
}

/// Decompress GLZA-compressed `compressed`.
///
/// # Errors
///
/// Returns [`OmnizipError::Corrupt`] on a malformed payload,
/// [`OmnizipError::DecodeFailed`] on a length mismatch, or
/// [`OmnizipError::LengthMismatch`] if the expanded output length differs
/// from the header's `uncompressed_size`.
pub fn decompress(compressed: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    let (uncompressed_size, start_rule, rules) = decode::parse(compressed)?;
    decode::expand(uncompressed_size, &start_rule, &rules)
}

/// GLZA codec adapter implementing the omnizip-codecs `Codec` trait.
#[derive(Clone, Copy, Debug, Default)]
pub struct GlzaCodec;

impl GlzaCodec {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Codec for GlzaCodec {
    fn id(&self) -> CodecId {
        GLZA_CODEC_ID
    }

    fn name(&self) -> &'static str {
        "glza"
    }

    fn compress(
        &self,
        plaintext: &[u8],
        _level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        compress(plaintext)
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let out = decompress(compressed)?;
        if out.len() as u32 != expected_len {
            return Err(OmnizipError::LengthMismatch {
                codec: GLZA_CODEC_ID,
                expected: expected_len,
                actual: out.len(),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss, clippy::len_zero)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let compressed = compress(b"").expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, b"");
    }

    #[test]
    fn round_trip_single_byte() {
        let compressed = compress(b"X").expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, b"X");
    }

    #[test]
    fn round_trip_short_text() {
        let input = b"hello world";
        let compressed = compress(input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_repetitive_text() {
        let input: Vec<u8> = b"the quick brown fox jumps over the lazy dog. ".repeat(50);
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_all_same_byte() {
        let input = vec![0x41u8; 5_000];
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_random_data() {
        // Pseudo-random data with no long repeats.
        let input: Vec<u8> = (0..5_000).map(|i| ((i * 7919) % 251) as u8).collect();
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_dna_like() {
        // DNA-like: only 4 distinct bytes, lots of repetition.
        let alphabet = [b'A', b'C', b'G', b'T'];
        let input: Vec<u8> = (0..4_000).map(|i| alphabet[(i * 17 + 3) % 4]).collect();
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_xml_like() {
        let input: Vec<u8> = b"<tag><child>data</child><child>data</child></tag>".repeat(100);
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_with_high_bytes() {
        // Input containing the 0xFF marker byte.
        let input: Vec<u8> = (0..=255u8).cycle().take(2_000).collect();
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn compresses_repetitive_data() {
        let input: Vec<u8> = b"<html><body>Hello, World!</body></html>".repeat(500);
        let compressed = compress(&input).expect("compress");
        let ratio = compressed.len() as f64 / input.len() as f64;
        assert!(
            compressed.len() < input.len(),
            "should compress repetitive data, ratio {ratio:.3}"
        );
    }

    #[test]
    fn ratio_target_on_dna() {
        let alphabet = [b'A', b'C', b'G', b'T'];
        let input: Vec<u8> = (0..10_000).map(|i| alphabet[(i * 17 + 3) % 4]).collect();
        let compressed = compress(&input).expect("compress");
        let ratio = compressed.len() as f64 / input.len() as f64;
        // Target: better than ~50% on DNA-like data.
        assert!(
            ratio < 0.6,
            "DNA ratio {ratio:.3} should be < 0.6 for Phase 1"
        );
    }

    #[test]
    fn codec_trait_round_trips() {
        let codec = GlzaCodec::new();
        let input = b"repetitive repetitive repetitive repetitive text".to_vec();
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .expect("compress");
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn determinism() {
        let input: Vec<u8> =
            b"the quick brown fox the quick brown fox the quick brown fox".to_vec();
        let a = compress(&input).expect("compress");
        let b = compress(&input).expect("compress");
        assert_eq!(a, b, "GLZA must be deterministic");
    }

    #[test]
    fn rejects_bad_magic() {
        let result = decompress(b"NOTGLZA\0\0\0\0\0");
        assert!(result.is_err());
    }

    #[test]
    fn codec_id_is_0x0d() {
        assert_eq!(GLZA_CODEC_ID.as_u16(), 0x000D);
        let codec = GlzaCodec::new();
        assert_eq!(codec.id().as_u16(), 0x000D);
        assert_eq!(codec.name(), "glza");
    }
}
