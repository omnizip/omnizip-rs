//! Cross-run determinism property tests.
//!
//! Runs each encoder multiple times with the same input and verifies
//! byte-identical output. This catches:
//!
//! - Hidden HashMap iteration (random order per run).
//! - Hidden time-seeded RNG.
//! - Hidden pointer-address dependencies.
//!
//! See ADR-0004 for the determinism requirement.

use omnizip_codecs::{Codec, CompressionLevel};

fn text_input() -> Vec<u8> {
    b"the quick brown fox jumps over the lazy dog. ".repeat(100)
}

fn csv_input() -> Vec<u8> {
    let mut data = Vec::new();
    for i in 0..500 {
        data.extend_from_slice(
            format!("{i},user_{i},city_{},cc,{}\n", i % 100, i % 1000).as_bytes(),
        );
    }
    data
}

fn binary_input() -> Vec<u8> {
    (0..4096u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 16) as u8)
        .collect()
}

#[test]
fn brotli_deterministic_across_runs() {
    let codec = omnizip_brotli::BrotliCodec::new();
    let level = CompressionLevel::new(5);
    for input in [text_input(), csv_input(), binary_input()] {
        let mut outputs = Vec::new();
        for _ in 0..5 {
            outputs.push(codec.compress(&input, level).expect("compress"));
        }
        for w in outputs.windows(2) {
            assert_eq!(w[0], w[1], "brotli not deterministic");
        }
    }
}

#[test]
fn zstd_deterministic_across_runs() {
    let codec = omnizip_zstd::ZstdCodec::new();
    let level = CompressionLevel::new(9);
    for input in [text_input(), csv_input(), binary_input()] {
        let mut outputs = Vec::new();
        for _ in 0..5 {
            outputs.push(codec.compress(&input, level).expect("compress"));
        }
        for w in outputs.windows(2) {
            assert_eq!(w[0], w[1], "zstd not deterministic");
        }
    }
}

#[test]
fn lzma_deterministic_across_runs() {
    let codec = omnizip_lzma::LzmaCodec::new();
    let level = CompressionLevel::new(5);
    for input in [text_input(), csv_input(), binary_input()] {
        let mut outputs = Vec::new();
        for _ in 0..5 {
            outputs.push(codec.compress(&input, level).expect("compress"));
        }
        for w in outputs.windows(2) {
            assert_eq!(w[0], w[1], "lzma not deterministic");
        }
    }
}
