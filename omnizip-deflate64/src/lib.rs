//! Pure-Rust Deflate64 codec — port of omnizip's Ruby reference at
//! `omnizip/lib/omnizip/algorithms/deflate64/` (5 files, 783 LOC).
//!
//! Deflate64 is Microsoft's enhanced variant of DEFLATE (ZIP method 9).
//! The defining differences from RFC 1951 DEFLATE:
//!
//! - **64 KB sliding window** (vs 32 KB)
//! - **Larger match distances** — up to 65 536 (vs 32 768)
//! - Distance code 29 absorbs the entire 32 KB+1 ..= 64 KB range
//!
//! Everything else — canonical Huffman coding, LZ77, the block structure —
//! is shared with standard DEFLATE.
//!
//! This crate implements the [`omnizip_codecs::Codec`] trait so it drops
//! into the omnizip-rs registry. The Phase 1 container format is the Ruby
//! reference's: two serialised Huffman tables followed by the coded
//! bitstream. Standard DEFLATE / zlib compatibility lands in a later phase.
//!
//! # Determinism
//!
//! Identical input + level always produces byte-identical output. The
//! Huffman construction uses stable sort with deterministic tie-breaking;
//! the hash chain is walked in insertion order.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod constants;
mod container;
pub mod decoder;
pub mod encoder;
pub mod huffman;
pub mod token;

pub use decoder::{DecodeError, Decoder};
pub use encoder::{Encoded, Encoder};
pub use huffman::{HuffCode, HuffTable, InverseTable};
pub use token::Token;

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// Minimum supported compression level.
pub const MIN_LEVEL: u8 = 0;
/// Maximum supported compression level.
pub const MAX_LEVEL: u8 = 9;

/// Deflate64 codec. Levels 0–9 are accepted; in Phase 1 the level only
/// selects the match-search aggressiveness indirectly via the encoder's
/// fixed chain length.
pub struct Deflate64Codec;

impl Codec for Deflate64Codec {
    fn id(&self) -> CodecId {
        CodecId::DEFLATE64
    }
    fn name(&self) -> &'static str {
        "deflate64"
    }
    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        let raw = level.as_u8();
        if !(MIN_LEVEL..=MAX_LEVEL).contains(&raw) {
            return Err(OmnizipError::LevelOutOfRange {
                codec: CodecId::DEFLATE64,
                level: raw,
                min: MIN_LEVEL,
                max: MAX_LEVEL,
            });
        }
        let enc = encoder::Encoder::new();
        let encoded = enc.encode(plaintext);
        Ok(container::pack(&encoded))
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::DEFLATE64,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;
        let (lit_table, dist_table, bitstream) =
            container::unpack(compressed).map_err(|reason| OmnizipError::Corrupt {
                codec: CodecId::DEFLATE64,
                reason,
            })?;
        match decoder::Decoder::decode(&lit_table, &dist_table, bitstream, expected_us) {
            Ok(out) => Ok(out),
            Err(decoder::DecodeError::Corrupt { reason }) => Err(OmnizipError::DecodeFailed {
                codec: CodecId::DEFLATE64,
                reason,
            }),
            Err(decoder::DecodeError::LengthMismatch { expected, actual }) => {
                Err(OmnizipError::LengthMismatch {
                    codec: CodecId::DEFLATE64,
                    expected: u32::try_from(expected).unwrap_or(u32::MAX),
                    actual,
                })
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    fn round_trip(data: &[u8]) {
        let compressed = Deflate64Codec
            .compress(data, CompressionLevel::default())
            .expect("compress");
        let decompressed = Deflate64Codec
            .decompress(&compressed, data.len() as u32)
            .expect("decompress");
        assert_eq!(
            decompressed,
            data,
            "round-trip mismatch for input of {} bytes",
            data.len()
        );
    }

    #[test]
    fn round_trip_empty() {
        round_trip(b"");
    }

    #[test]
    fn round_trip_short_text() {
        round_trip(b"Hello, World!");
    }

    #[test]
    fn round_trip_long_text() {
        let data = b"The quick brown fox jumps over the lazy dog. ".repeat(500);
        round_trip(&data);
    }

    #[test]
    fn round_trip_repetitive_pattern() {
        let data = b"Hello, World! ".repeat(100);
        round_trip(&data);
    }

    #[test]
    fn round_trip_binary() {
        let data: Vec<u8> = (0..1024u32).map(|i| (i * 7) as u8).collect();
        round_trip(&data);
    }

    #[test]
    fn round_trip_random_incompressible() {
        // Pseudo-random but deterministic so the test is reproducible.
        // LZ77 rarely finds 3-byte matches in random data, so this exercises
        // the literal-heavy code path.
        let mut state: u32 = 0x1234_5678;
        let data: Vec<u8> = (0..2000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state & 0xFF) as u8
            })
            .collect();
        round_trip(&data);
    }

    #[test]
    fn compresses_repetitive_data() {
        let data = vec![0x41u8; 10_000];
        let compressed = Deflate64Codec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        assert!(
            compressed.len() < data.len(),
            "expected compression, got {} -> {}",
            data.len(),
            compressed.len()
        );
    }

    #[test]
    fn deterministic_output() {
        let data = b"Lorem ipsum dolor sit amet. ".repeat(200);
        let a = Deflate64Codec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        let b = Deflate64Codec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        assert_eq!(a, b, "two compress calls produced different output");
    }

    #[test]
    fn rejects_out_of_range_level() {
        let result = Deflate64Codec.compress(b"x", CompressionLevel::new(10));
        assert!(matches!(
            result,
            Err(OmnizipError::LevelOutOfRange {
                codec: CodecId::DEFLATE64,
                level: 10,
                min: 0,
                max: 9
            })
        ));
    }

    #[test]
    fn rejects_truncated_container() {
        let result = Deflate64Codec.decompress(b"\x00\x00\x00\x01", 100);
        assert!(result.is_err());
    }

    #[test]
    fn level_range_accepted() {
        let data = b"round and round and round we go. ".repeat(50);
        for lvl in MIN_LEVEL..=MAX_LEVEL {
            let compressed = Deflate64Codec
                .compress(&data, CompressionLevel::new(lvl))
                .expect("compress");
            let decompressed = Deflate64Codec
                .decompress(&compressed, data.len() as u32)
                .expect("decompress");
            assert_eq!(decompressed, data, "level {lvl} round-trip");
        }
    }
}
