//! Differential parity for `omnizip_lzma::lzma_alone_decompress` against
//! the reference `xz -d` oracle.
//!
//! Each `.lzma` fixture under `tests/fixtures/lzma/good-*.lzma` is
//! decoded twice: once by our Rust port, once by `xz -d`. The bytes
//! must match exactly.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use std::fs;
use std::path::PathBuf;

use omnizip_differential::xz_oracle_decode;
use omnizip_lzma::lzma_alone_decompress;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("lzma")
}

fn run_parity(filename: &str) {
    let fixture = fixtures_dir().join(filename);
    if !fixture.exists() {
        eprintln!("skipped (fixture missing): {filename}");
        return;
    }
    let compressed =
        fs::read(&fixture).unwrap_or_else(|e| panic!("read {}: {e}", fixture.display()));

    let oracle = if let Some(o) = xz_oracle_decode(&fixture).expect("oracle invocation") { o.bytes } else {
        eprintln!("skipped (no xz oracle): {filename}");
        return;
    };

    let rust = lzma_alone_decompress(&compressed).expect("rust decode");

    assert_eq!(
        rust, oracle,
        "{}: byte mismatch — Rust {} bytes, oracle {} bytes",
        filename,
        rust.len(),
        oracle.len()
    );
}

#[test]
fn good_known_size_with_eopm() {
    run_parity("good-known_size-with_eopm.lzma");
}

#[test]
fn good_known_size_without_eopm() {
    run_parity("good-known_size-without_eopm.lzma");
}

#[test]
fn good_unknown_size_with_eopm() {
    run_parity("good-unknown_size-with_eopm.lzma");
}
