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
mod wire;

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
        // Wire-first for foreign Deflate64 streams (ZIP method 9);
        // the legacy Ruby-invented container parse is kept as a
        // fallback for streams this codec itself produced before the
        // wire layer existed. A real wire stream cannot parse as the
        // container (its BE32 length fields never match), and a
        // legacy stream fails wire decoding structurally.
        let legacy = container::unpack(compressed).and_then(|(l, d, b)| {
            decoder::Decoder::decode(&l, &d, b, expected_us)
                .map_err(|_| "legacy decode failed".to_string())
        });
        match legacy {
            Ok(out) => Ok(out),
            Err(_) => wire::inflate64(compressed).map_err(|reason| OmnizipError::Corrupt {
                codec: CodecId::DEFLATE64,
                reason,
            }),
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

#[test]
fn decodes_real_deflate64_wire_stream() {
    // Ground truth produced by 7-Zip (`7zz a -mm=Deflate64`), a
    // 172-byte input compressed to 44 bytes — the standing guard for
    // the wire-format layer after the interop probe found the legacy
    // container never matched real Deflate64.
    let member = [
        0xc5, 0xca, 0x49, 0x0d, 0x00, 0x20, 0x0c, 0x04, 0x40, 0x2b, 0xab, 0x03, 0x37, 0x2c, 0xe9,
        0xa3, 0x09, 0x57, 0x68, 0x79, 0xe0, 0x1e, 0x01, 0x15, 0xd0, 0xcc, 0x77, 0x5c, 0xe7, 0x03,
        0xaf, 0xa3, 0xad, 0xb1, 0x8f, 0x98, 0x29, 0xbb, 0x14, 0x54, 0x46, 0xe9, 0xf5, 0x03,
    ];
    let want: Vec<u8> = b"tiny but compressible: abababababababababab"
        .iter()
        .copied()
        .cycle()
        .take(172)
        .collect();
    let out = Deflate64Codec
        .decompress(&member, want.len() as u32)
        .expect("wire stream decodes");
    assert_eq!(out, want);
}
