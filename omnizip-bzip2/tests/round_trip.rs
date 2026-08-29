//! Integration tests for the `BZip2` codec round-trip behaviour.
//!
//! Casts below are on known-small values (test fixture sizes, level bytes);
//! the pedantic truncation lint would just add noise.

#![allow(clippy::cast_possible_truncation)]

use omnizip_bzip2::Bzip2Codec;
use omnizip_codecs::{Codec, CompressionLevel};

fn codec() -> Bzip2Codec {
    Bzip2Codec::new()
}

fn round_trip(data: &[u8], level: u8) {
    let c = codec();
    let compressed = c
        .compress(data, CompressionLevel::new(level))
        .unwrap_or_else(|e| panic!("compress failed at level {level}: {e}"));
    let decompressed = c
        .decompress(&compressed, data.len() as u32)
        .unwrap_or_else(|e| panic!("decompress failed: {e}"));
    assert_eq!(
        decompressed,
        data,
        "round-trip mismatch at level {level} (len={})",
        data.len()
    );
}

#[test]
fn empty_input() {
    let c = codec();
    let compressed = c
        .compress(b"", CompressionLevel::new(9))
        .expect("compress empty");
    // Empty input produces a valid empty .bz2 member (like `bzip2`).
    assert_eq!(&compressed[..3], b"BZh");
    let decompressed = c.decompress(&compressed, 0).expect("decompress empty");
    assert!(decompressed.is_empty());
}

#[test]
fn single_byte() {
    round_trip(b"X", 1);
    round_trip(b"X", 9);
}

#[test]
fn short_text() {
    round_trip(b"Hello, BZip2!", 1);
    round_trip(b"Hello, BZip2!", 5);
    round_trip(b"Hello, BZip2!", 9);
}

#[test]
fn long_text() {
    let data = b"The quick brown fox jumps over the lazy dog. ".repeat(500);
    round_trip(&data, 1);
    round_trip(&data, 6);
    round_trip(&data, 9);
}

#[test]
fn highly_repetitive() {
    // BWT should shine here.
    let data: Vec<u8> = std::iter::repeat(b'A').take(10_000).collect();
    round_trip(&data, 9);
    let compressed = codec()
        .compress(&data, CompressionLevel::new(9))
        .expect("compress");
    assert!(
        compressed.len() < data.len(),
        "expected compression on repetitive data: {} vs {}",
        compressed.len(),
        data.len()
    );
}

#[test]
fn binary_data() {
    let data: Vec<u8> = (0..=255u32).cycle().take(10_000).map(|i| i as u8).collect();
    round_trip(&data, 1);
    round_trip(&data, 9);
}

#[test]
fn mixed_content() {
    // Mix of text, repetition, and binary — stresses every pipeline stage.
    let mut data = Vec::new();
    data.extend_from_slice(b"Some text here. ");
    data.extend(std::iter::repeat(0x00u8).take(1000));
    data.extend_from_slice(b"More text! ");
    data.extend((0..=255u32).map(|i| i as u8));
    data.extend(std::iter::repeat(0xFFu8).take(500));
    round_trip(&data, 9);
}

#[test]
fn compresses_text_better_than_raw() {
    let data = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
                 bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\
                 ccccccccccccccccccccccccccccccccccccccccccc"
        .to_vec();
    let compressed = codec()
        .compress(&data, CompressionLevel::new(9))
        .expect("compress");
    assert!(
        compressed.len() < data.len(),
        "expected compressed < raw: {} vs {}",
        compressed.len(),
        data.len()
    );
}

#[test]
fn determinism_same_input_same_output() {
    let data = b"The quick brown fox jumps over the lazy dog. ".repeat(100);
    let c = codec();
    let a = c
        .compress(&data, CompressionLevel::new(6))
        .expect("compress a");
    let b = c
        .compress(&data, CompressionLevel::new(6))
        .expect("compress b");
    assert_eq!(
        a, b,
        "non-deterministic output: same input produced different compressed bytes"
    );
}

#[test]
fn multi_block_round_trip() {
    // Force multiple blocks at level 1 (100_000-byte blocks). Keep input
    // modest so the BWT completes quickly.
    let data: Vec<u8> = (0..120_000u32).map(|i| (i % 251) as u8).collect();
    round_trip(&data, 1);
}

#[test]
fn rejects_bad_expected_len() {
    let c = codec();
    let data = b"hello world";
    let compressed = c
        .compress(data, CompressionLevel::new(9))
        .expect("compress");
    // Lie about expected length.
    let result = c.decompress(&compressed, (data.len() + 1) as u32);
    assert!(result.is_err(), "should reject wrong expected_len");
}

#[test]
fn all_levels_round_trip() {
    let data = b"Patchie is a good dog and she likes to run around the yard. ".repeat(50);
    for level in 1..=9u8 {
        round_trip(&data, level);
    }
}

#[test]
fn truncated_input_errors() {
    let c = codec();
    let data = b"some data here that is long enough";
    let mut compressed = c
        .compress(data, CompressionLevel::new(9))
        .expect("compress");
    // Truncate the bit stream.
    compressed.truncate(compressed.len() - 1);
    let result = c.decompress(&compressed, data.len() as u32);
    assert!(result.is_err(), "should reject truncated input");
}

/// Check whether the reference `bzip2` CLI is available on PATH.
fn system_bzip2_available() -> bool {
    std::process::Command::new("bzip2")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Our `.bz2` output must decode byte-identically in the reference
/// implementation. Regression gate for two bugs found by the
/// broad-corpus sweep: blocks whose RLE1 stream exceeded the wire
/// budget (bzip2: data-integrity error on low-redundancy data), and
/// single-table Huffman losing 5-23% ratio.
#[test]
fn interop_with_system_bzip2() {
    if !system_bzip2_available() {
        eprintln!("skipping: system bzip2 not found");
        return;
    }
    let mut periodic = Vec::new();
    for i in 0..2600u32 {
        periodic
            .extend_from_slice(format!("{i},user_{i},city_{i},cc,{i},-180.0,0.0,{i}\n").as_bytes());
    }
    periodic.extend(std::iter::repeat(b'z').take(700));
    let inputs: Vec<&[u8]> = vec![b"hello hello hello world", &periodic, &[0u8; 10_000]];
    for input in inputs {
        for lv in [1u8, 9] {
            let compressed = codec()
                .compress(input, CompressionLevel::new(lv))
                .unwrap_or_else(|e| panic!("encode lv{lv}: {e}"));
            let out = std::process::Command::new("bzip2")
                .arg("-d")
                .arg("-c")
                .arg("-")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut c| {
                    use std::io::Write as _;
                    c.stdin.take().unwrap().write_all(&compressed)?;
                    c.wait_with_output().map_err(std::io::Error::other)
                });
            let out = match out {
                Ok(o) => o,
                Err(e) => panic!("spawn bzip2 lv{lv}: {e}"),
            };
            assert!(
                out.status.success(),
                "bzip2 -d rejected our output (lv{lv}, {} B input): {}",
                input.len(),
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                &out.stdout[..],
                input,
                "bzip2 -d decoded data mismatch (lv{lv})"
            );
        }
    }
}
