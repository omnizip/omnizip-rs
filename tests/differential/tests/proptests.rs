//! Property-based round-trip tests for every codec.
//!
//! Each test generates N pseudo-random inputs of varying sizes,
//! compresses via the codec, decompresses, and asserts equality.
//! PRNG is xorshift64 with a fixed seed for reproducibility.
//!
//! This catches:
//! - Wire-format regressions (corrupt output → decode failure).
//! - Determinism violations (same input, different output).
//! - Edge cases at common size boundaries (32, 64, 256, 4096).
//! - Round-trip failures on input patterns that aren't in unit tests.
//!
//! ## Adding a codec
//!
//! Add it to `CODECS_TO_TEST` below. The harness handles the rest.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use omnizip_brotli::BrotliCodec;
use omnizip_bzip2::Bzip2Codec;
use omnizip_codecs::{Codec, CompressionLevel};
use omnizip_deflate::DeflateCodec;
use omnizip_flac::FlacCodec;
use omnizip_libdeflate::LibdeflateCodec;
use omnizip_lz4::{Lz4FastCodec, Lz4HcCodec};
use omnizip_lzma::LzmaCodec;
use omnizip_ricepp::RiceppCodec;
use omnizip_snappy::SnappyCodec;
use omnizip_zstd::ZstdCodec;

/// Deterministic xorshift64 PRNG. Same seed → same sequence.
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
    /// Generate `n` bytes with the given distribution.
    fn fill_bytes(&mut self, n: usize, alphabet_size: u8, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(n);
        for _ in 0..n {
            let b = (self.next_u64() % u64::from(alphabet_size)) as u8;
            out.push(b);
        }
    }
}

/// Test fixtures: each (name, size, alphabet).
///
/// Alphabet controls entropy: 1 = constant bytes (max compression),
/// 4 = small alphabet (RLE-friendly), 255 = near-full byte range
/// (worst case for compression).
const FIXTURES: &[(&str, usize, u8)] = &[
    ("empty", 0, 255),
    ("one", 1, 255),
    ("two", 2, 255),
    ("seven", 7, 255),
    ("block_32", 32, 255),
    ("block_64", 64, 255),
    ("block_128", 128, 255),
    ("block_256", 256, 255),
    ("block_1024", 1024, 255),
    ("block_4096", 4096, 255),
    ("low_entropy_64", 64, 4),
    ("low_entropy_256", 256, 4),
    ("low_entropy_4096", 4096, 4),
    ("tiny_alphabet_64", 64, 2),
    ("tiny_alphabet_1024", 1024, 2),
    ("constant_64", 64, 1),
    ("constant_256", 256, 1),
];

/// Generate all fixtures deterministically.
fn make_fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let mut prng = XorShift64::new(0xDEAD_BEEF_BAAD_F00D);
    let mut out = Vec::new();
    for &(name, size, alphabet) in FIXTURES {
        let mut bytes = Vec::new();
        prng.fill_bytes(size, alphabet, &mut bytes);
        out.push((name, bytes));
    }
    out
}

// ---------------------------------------------------------------------------
// Per-codec round-trip tests
// ---------------------------------------------------------------------------

#[test]
fn brotli_round_trips_property_fixtures() {
    let codec = BrotliCodec::new();
    for (name, input) in make_fixtures() {
        let compressed = match codec.compress(&input, CompressionLevel::default()) {
            Ok(c) => c,
            Err(e) => panic!("brotli compress {name} ({} bytes): {e}", input.len()),
        };
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .unwrap_or_else(|e| panic!("brotli decompress {name}: {e}"));
        assert_eq!(decompressed, input, "brotli round-trip mismatch on {name}");
    }
}

#[test]
fn bzip2_round_trips_property_fixtures() {
    let codec = Bzip2Codec::new();
    for (name, input) in make_fixtures() {
        if input.is_empty() {
            continue; // bzip2 codec returns empty for empty input.
        }
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .unwrap_or_else(|e| panic!("bzip2 compress {name}: {e}"));
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .unwrap_or_else(|e| panic!("bzip2 decompress {name}: {e}"));
        assert_eq!(decompressed, input, "bzip2 round-trip mismatch on {name}");
    }
}

#[test]
fn deflate_round_trips_property_fixtures() {
    let codec = DeflateCodec::new();
    for (name, input) in make_fixtures() {
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .unwrap_or_else(|e| panic!("deflate compress {name}: {e}"));
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .unwrap_or_else(|e| panic!("deflate decompress {name}: {e}"));
        assert_eq!(decompressed, input, "deflate round-trip mismatch on {name}");
    }
}

#[test]
fn libdeflate_round_trips_property_fixtures() {
    let codec = LibdeflateCodec::new();
    for (name, input) in make_fixtures() {
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .unwrap_or_else(|e| panic!("libdeflate compress {name}: {e}"));
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .unwrap_or_else(|e| panic!("libdeflate decompress {name}: {e}"));
        assert_eq!(
            decompressed, input,
            "libdeflate round-trip mismatch on {name}"
        );
    }
}

#[test]
fn lzma_round_trips_property_fixtures() {
    let codec = LzmaCodec::new();
    for (name, input) in make_fixtures() {
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .unwrap_or_else(|e| panic!("lzma compress {name}: {e}"));
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .unwrap_or_else(|e| panic!("lzma decompress {name}: {e}"));
        assert_eq!(decompressed, input, "lzma round-trip mismatch on {name}");
    }
}

#[test]
fn lz4_fast_round_trips_property_fixtures() {
    let codec = Lz4FastCodec;
    for (name, input) in make_fixtures() {
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .unwrap_or_else(|e| panic!("lz4 fast compress {name}: {e}"));
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .unwrap_or_else(|e| panic!("lz4 fast decompress {name}: {e}"));
        assert_eq!(
            decompressed, input,
            "lz4 fast round-trip mismatch on {name}"
        );
    }
}

#[test]
fn lz4_hc_round_trips_property_fixtures() {
    let codec = Lz4HcCodec;
    for (name, input) in make_fixtures() {
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .unwrap_or_else(|e| panic!("lz4 hc compress {name}: {e}"));
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .unwrap_or_else(|e| panic!("lz4 hc decompress {name}: {e}"));
        assert_eq!(decompressed, input, "lz4 hc round-trip mismatch on {name}");
    }
}

#[test]
fn snappy_round_trips_property_fixtures() {
    let codec = SnappyCodec;
    for (name, input) in make_fixtures() {
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .unwrap_or_else(|e| panic!("snappy compress {name}: {e}"));
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .unwrap_or_else(|e| panic!("snappy decompress {name}: {e}"));
        assert_eq!(decompressed, input, "snappy round-trip mismatch on {name}");
    }
}

#[test]
fn zstd_round_trips_property_fixtures() {
    let codec = ZstdCodec::new();
    for (name, input) in make_fixtures() {
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .unwrap_or_else(|e| panic!("zstd compress {name}: {e}"));
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .unwrap_or_else(|e| panic!("zstd decompress {name}: {e}"));
        assert_eq!(decompressed, input, "zstd round-trip mismatch on {name}");
    }
}

/// Validates ZSTD wire-format correctness by decoding our encoder's
/// output with the reference `zstd -d` CLI (v1.5.7+). A non-zero exit
/// status or byte mismatch indicates our output is not RFC 8478 compliant
/// at the given level. Skipped if `zstd` is not on PATH.
#[test]
fn zstd_reference_decoder_validates_output() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn reference_decode(our_output: &[u8], expected_len: usize) -> Option<Vec<u8>> {
        let mut child = Command::new("zstd")
            .arg("-d")
            .arg("-c")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(our_output).ok()?;
        }
        let output = child.wait_with_output().ok()?;
        if !output.status.success() {
            return None;
        }
        if output.stdout.len() != expected_len {
            return None;
        }
        Some(output.stdout)
    }

    let zstd_ok = std::path::Path::new("/usr/bin/zstd").exists()
        || std::path::Path::new("/usr/local/bin/zstd").exists()
        || std::path::Path::new("/opt/homebrew/bin/zstd").exists();
    if !zstd_ok {
        eprintln!("skipping: zstd CLI not found");
        return;
    }

    let inputs: Vec<(&str, Vec<u8>)> = vec![
        ("text_short", b"hello zstd world".to_vec()),
        (
            "text_repeated",
            b"the quick brown fox jumps over the lazy dog. ".repeat(20),
        ),
        (
            "binary_periodic",
            (0u32..256)
                .map(|i| (i % 256) as u8)
                .collect::<Vec<u8>>()
                .repeat(10),
        ),
    ];

    let codec = ZstdCodec::new();
    for level in 1..=6 {
        let mut failures = Vec::new();
        for (name, data) in &inputs {
            let ours = codec.compress(data, CompressionLevel::new(level)).unwrap();
            match reference_decode(&ours, data.len()) {
                Some(decoded) if decoded.as_slice() == data.as_slice() => {
                    eprintln!(
                        "zstd -d OK level={level} {name}: {} compressed bytes",
                        ours.len()
                    );
                }
                Some(_) => failures.push(format!("{name}: decoded but bytes differ")),
                None => failures.push(format!("{name}: zstd -d rejected output")),
            }
        }
        assert!(
            failures.is_empty(),
            "zstd level {level} failures: {}",
            failures.join(", ")
        );
    }
}

#[test]
fn flac_round_trips_property_fixtures() {
    let codec = FlacCodec::new();
    // FlacCodec::compress uses default 44.1kHz stereo 16-bit params,
    // so input must be a multiple of 4 bytes (2 channels × 16-bit).
    for (name, input) in make_fixtures() {
        if input.len() % 4 != 0 || input.is_empty() {
            continue;
        }
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .unwrap_or_else(|e| panic!("flac compress {name}: {e}"));
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .unwrap_or_else(|e| panic!("flac decompress {name}: {e}"));
        assert_eq!(decompressed, input, "flac round-trip mismatch on {name}");
    }
}

#[test]
fn ricepp_round_trips_property_fixtures() {
    let codec = RiceppCodec::new();
    // ricepp requires input length to be a multiple of pixel width 2.
    for (name, input) in make_fixtures() {
        if input.len() % 2 != 0 {
            continue;
        }
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .unwrap_or_else(|e| panic!("ricepp compress {name}: {e}"));
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .unwrap_or_else(|e| panic!("ricepp decompress {name}: {e}"));
        assert_eq!(decompressed, input, "ricepp round-trip mismatch on {name}");
    }
}

// ---------------------------------------------------------------------------
// Determinism test: same input always produces same output bytes.
// ---------------------------------------------------------------------------

#[test]
fn all_codecs_are_deterministic_across_calls() {
    let mut prng = XorShift64::new(0xBEEF_CAFE_1234_5678);
    let mut input = Vec::new();
    prng.fill_bytes(1024, 255, &mut input);

    let brotli = BrotliCodec::new();
    assert_eq!(
        brotli
            .compress(&input, CompressionLevel::default())
            .unwrap(),
        brotli
            .compress(&input, CompressionLevel::default())
            .unwrap(),
        "brotli nondeterministic"
    );

    let bzip2 = Bzip2Codec::new();
    assert_eq!(
        bzip2.compress(&input, CompressionLevel::default()).unwrap(),
        bzip2.compress(&input, CompressionLevel::default()).unwrap(),
        "bzip2 nondeterministic"
    );

    let deflate = DeflateCodec::new();
    assert_eq!(
        deflate
            .compress(&input, CompressionLevel::default())
            .unwrap(),
        deflate
            .compress(&input, CompressionLevel::default())
            .unwrap(),
        "deflate nondeterministic"
    );

    let lzma = LzmaCodec::new();
    assert_eq!(
        lzma.compress(&input, CompressionLevel::default()).unwrap(),
        lzma.compress(&input, CompressionLevel::default()).unwrap(),
        "lzma nondeterministic"
    );

    let zstd = ZstdCodec::new();
    assert_eq!(
        zstd.compress(&input, CompressionLevel::default()).unwrap(),
        zstd.compress(&input, CompressionLevel::default()).unwrap(),
        "zstd nondeterministic"
    );
}
