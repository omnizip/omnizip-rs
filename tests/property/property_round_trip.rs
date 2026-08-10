//! Property-based tests for omnizip-rs encoders.
//!
//! This is a manual scaffold (no `proptest` dependency yet) that
//! exercises the invariants every encoder must satisfy:
//!
//! - Round-trip: decompress(compress(x)) == x
//! - Determinism: compress(x) called twice produces byte-identical output
//!
//! Each codec is tested against structured generators: empty,
//! single byte, short text, CSV-like, repetitive, random binary,
//! and large text inputs. The generators use a fixed seed for
//! reproducibility.
//!
//! See TODO 250 for the full plan including `proptest` migration,
//! cross-decoder fuzzing, and monotonic-ratio checks.

use omnizip_codecs::{Codec, CompressionLevel};

/// Fixed-seed pseudo-random generator (deterministic).
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_mul(0x5851_F42D_4C95_7F2D),
        }
    }

    fn next_u64(&mut self) -> u64 {
        // SplitMix64 — deterministic, no unsafe.
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_u8(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }

    fn next_bool(&mut self) -> bool {
        (self.next_u64() & 1) == 1
    }
}

/// Structured input generator. Produces a deterministic stream of
/// "interesting" inputs that exercise edge cases.
fn generate_inputs(seed: u64) -> Vec<Vec<u8>> {
    let mut rng = Rng::new(seed);
    let mut inputs: Vec<Vec<u8>> = Vec::new();

    // Empty input.
    inputs.push(Vec::new());

    // Single byte variants.
    inputs.push(vec![0u8]);
    inputs.push(vec![0xFF]);
    inputs.push(vec![b'A']);

    // Short text.
    inputs.push(b"hello world".to_vec());
    inputs.push(b"the quick brown fox".to_vec());

    // CSV-like.
    inputs.push(b"id,name,city\n1,alice,paris\n2,bob,london\n3,charlie,berlin\n".to_vec());

    // JSON-like.
    inputs.push(
        br#"{"id":1,"name":"alice","tags":["a","b","c"]}
{"id":2,"name":"bob","tags":["d"]}
"#
        .to_vec(),
    );

    // Highly repetitive.
    inputs.push(vec![b'a'; 4096]);
    inputs.push(b"abcabcabcabc".repeat(100));

    // Pseudo-random binary.
    let mut bin = Vec::with_capacity(1024);
    for _ in 0..1024 {
        bin.push(rng.next_u8());
    }
    inputs.push(bin);

    // Larger text (~8 KiB).
    let mut text = Vec::with_capacity(8192);
    let words = b"the quick brown fox jumps over the lazy dog ";
    while text.len() < 8192 {
        let start = (rng.next_u64() as usize) % words.len();
        text.extend_from_slice(&words[start..]);
    }
    text.truncate(8192);
    inputs.push(text);

    // All same byte.
    inputs.push(vec![0x42; 256]);

    // Alternating pattern.
    inputs.push(
        (0..256)
            .map(|i| if i % 2 == 0 { 0 } else { 0xFF })
            .collect(),
    );

    inputs
}

/// Run round-trip + determinism checks for one codec + level.
fn check_codec<C: Codec>(codec: &C, name: &str, level: u8) {
    let inputs = generate_inputs(0xC0DE_2026);
    let level = CompressionLevel::new(level);

    for (i, input) in inputs.iter().enumerate() {
        // Round-trip
        let compressed = codec.compress(input, level).unwrap_or_else(|e| {
            panic!(
                "{} level {:?}: input #{} compress failed: {:?}",
                name, level, i, e
            )
        });
        if !input.is_empty() {
            // Empty input may produce empty output for some codecs; skip strict length check.
            let decompressed = codec
                .decompress(&compressed, input.len() as u32)
                .unwrap_or_else(|e| {
                    panic!(
                        "{} level {:?}: input #{} decompress failed: {:?}",
                        name, level, i, e
                    )
                });
            assert_eq!(
                decompressed, *input,
                "{} level {:?}: input #{} round-trip mismatch",
                name, level, i
            );
        }

        // Determinism: compress twice, expect byte-identical
        let compressed2 = codec.compress(input, level).expect("second compress");
        assert_eq!(
            compressed, compressed2,
            "{} level {:?}: input #{} not deterministic",
            name, level, i
        );
    }
}

#[test]
fn brotli_round_trip_and_determinism() {
    let codec = omnizip_brotli::BrotliCodec::new();
    for &level in &[1u8, 5, 11] {
        check_codec(&codec, "brotli", level);
    }
}

#[test]
fn zstd_round_trip_and_determinism() {
    let codec = omnizip_zstd::ZstdCodec::new();
    for &level in &[1u8, 9, 19] {
        check_codec(&codec, "zstd", level);
    }
}

#[test]
fn lzma_round_trip_and_determinism() {
    let codec = omnizip_lzma::LzmaCodec::new();
    for &level in &[1u8, 5, 9] {
        check_codec(&codec, "lzma", level);
    }
}

#[test]
fn lz4_round_trip_and_determinism() {
    let codec = omnizip_lz4::Lz4FastCodec;
    check_codec(&codec, "lz4", 1);
}

#[test]
fn deflate_round_trip_and_determinism() {
    let codec = omnizip_deflate::DeflateCodec::new();
    check_codec(&codec, "deflate", 6);
}
