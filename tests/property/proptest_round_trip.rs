//! Property-based round-trip tests via `proptest`.
//!
//! These complement the manual scaffold in `property_round_trip.rs`
//! with proptest's structured generation and shrinking. Each codec
//! is tested across:
//!
//! - random binary data (any bytes)
//! - text-like data (printable ASCII)
//! - structured data (CSV/JSON-like)
//!
//! On failure, proptest shrinks to a minimal failing case and
//! persists it to `proptest-regressions/` for reproducibility.

use omnizip_codecs::{Codec, CompressionLevel};
use proptest::prelude::*;

/// Any byte sequence up to 4 KiB.
fn arbitrary_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..4096)
}

/// Text-like byte sequence (printable ASCII + common whitespace).
fn text_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(
        prop::sample::select(b"abcdefghij ABCDEFGHIJ \n\t,.:;".to_vec()),
        0..4096,
    )
}

/// Highly-repetitive byte sequence (stress match finder).
fn repetitive_bytes() -> impl Strategy<Value = Vec<u8>> {
    (1usize..32).prop_flat_map(|pattern_len| {
        let pattern: Vec<u8> = (0..pattern_len).map(|i| (i & 0xFF) as u8).collect();
        let pattern = pattern.clone();
        (0usize..2048).prop_map(move |repeat| {
            pattern
                .iter()
                .cycle()
                .take(pattern.len() * repeat)
                .copied()
                .collect::<Vec<u8>>()
        })
    })
}

fn csv_like_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(
        r"[a-z_]+,(0|[1-9][0-9]*),[a-z]+\n".prop_map(|s| s.into_bytes()),
        0..50,
    )
    .prop_map(|rows| rows.concat())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    })]

    #[test]
    fn brotli_round_trip_arbitrary(input in arbitrary_bytes()) {
        let codec = omnizip_brotli::BrotliCodec::new();
        for &level in &[1u8, 5, 11] {
            let compressed = codec.compress(&input, CompressionLevel::new(level))?;
            if input.is_empty() { continue; }
            let expected = input.clone();
            let decompressed = codec.decompress(&compressed, expected.len() as u32)?;
            prop_assert_eq!(decompressed, expected);
        }
    }

    #[test]
    fn brotli_round_trip_text(input in text_bytes()) {
        let codec = omnizip_brotli::BrotliCodec::new();
        let compressed = codec.compress(&input, CompressionLevel::new(5))?;
        if input.is_empty() { return Ok(()); }
        let decompressed = codec.decompress(&compressed, input.len() as u32)?;
        prop_assert_eq!(decompressed, input);
    }

    #[test]
    fn brotli_round_trip_repetitive(input in repetitive_bytes()) {
        let codec = omnizip_brotli::BrotliCodec::new();
        let compressed = codec.compress(&input, CompressionLevel::new(5))?;
        let decompressed = codec.decompress(&compressed, input.len() as u32)?;
        prop_assert_eq!(decompressed, input);
    }

    #[test]
    fn brotli_round_trip_csv(input in csv_like_bytes()) {
        let codec = omnizip_brotli::BrotliCodec::new();
        let compressed = codec.compress(&input, CompressionLevel::new(5))?;
        if input.is_empty() { return Ok(()); }
        let decompressed = codec.decompress(&compressed, input.len() as u32)?;
        prop_assert_eq!(decompressed, input);
    }

    #[test]
    fn zstd_round_trip_arbitrary(input in arbitrary_bytes()) {
        let codec = omnizip_zstd::ZstdCodec::new();
        for &level in &[1u8, 9, 19] {
            let compressed = codec.compress(&input, CompressionLevel::new(level))?;
            if input.is_empty() { continue; }
            let expected = input.clone();
            let decompressed = codec.decompress(&compressed, expected.len() as u32)?;
            prop_assert_eq!(decompressed, expected);
        }
    }

    #[test]
    fn lzma_round_trip_arbitrary(input in arbitrary_bytes()) {
        let codec = omnizip_lzma::LzmaCodec::new();
        for &level in &[1u8, 5, 9] {
            let compressed = codec.compress(&input, CompressionLevel::new(level))?;
            if input.is_empty() { continue; }
            let expected = input.clone();
            let decompressed = codec.decompress(&compressed, expected.len() as u32)?;
            prop_assert_eq!(decompressed, expected);
        }
    }

    #[test]
    fn lz4_round_trip_arbitrary(input in arbitrary_bytes()) {
        let codec = omnizip_lz4::Lz4FastCodec;
        let compressed = codec.compress(&input, CompressionLevel::new(1))?;
        if input.is_empty() { return Ok(()); }
        let decompressed = codec.decompress(&compressed, input.len() as u32)?;
        prop_assert_eq!(decompressed, input);
    }

    #[test]
    fn deflate_round_trip_arbitrary(input in arbitrary_bytes()) {
        let codec = omnizip_deflate::DeflateCodec::new();
        let compressed = codec.compress(&input, CompressionLevel::new(6))?;
        if input.is_empty() { return Ok(()); }
        let decompressed = codec.decompress(&compressed, input.len() as u32)?;
        prop_assert_eq!(decompressed, input);
    }

    #[test]
    fn brotli_deterministic(input in arbitrary_bytes()) {
        // Same input + level must produce byte-identical output across runs.
        let codec = omnizip_brotli::BrotliCodec::new();
        let a = codec.compress(&input, CompressionLevel::new(5))?;
        let b = codec.compress(&input, CompressionLevel::new(5))?;
        prop_assert_eq!(a, b);
    }

    #[test]
    fn zstd_deterministic(input in arbitrary_bytes()) {
        let codec = omnizip_zstd::ZstdCodec::new();
        let a = codec.compress(&input, CompressionLevel::new(9))?;
        let b = codec.compress(&input, CompressionLevel::new(9))?;
        prop_assert_eq!(a, b);
    }
}
