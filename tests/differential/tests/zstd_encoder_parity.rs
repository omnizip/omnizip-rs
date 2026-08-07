//! Encoder differential parity: encode via Rust, decode via `zstd -d`,
//! assert byte-identical round-trip.

#![forbid(unsafe_code)]

use omnizip_zstd::{compress, ZstdLevel};
use std::process::Command;

fn zstd_oracle_available() -> bool {
    Command::new("which")
        .arg("zstd")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn rust_encoder_round_trips_via_reference_decoder() {
    if !zstd_oracle_available() {
        eprintln!("skipped: zstd oracle unavailable");
        return;
    }
    let inputs: &[&[u8]] = &[
        b"",
        b"a",
        b"hello world",
        &vec![0x42u8; 1000],
        &vec![0x00u8; 4096],
        &(0..200_000).map(|i| (i % 251) as u8).collect::<Vec<u8>>(),
    ];
    for (i, input) in inputs.iter().enumerate() {
        let compressed = compress(input, ZstdLevel::Default).expect("encode");
        // Write to temp file, decode via zstd -d
        let path = std::env::temp_dir().join(format!("omnizip_zstd_parity_{i}.zst"));
        std::fs::write(&path, &compressed).expect("write");
        let out = Command::new("zstd")
            .arg("--decompress")
            .arg("--stdout")
            .arg(&path)
            .output()
            .expect("invoke zstd");
        assert!(
            out.status.success(),
            "zstd -d failed on input {i}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            out.stdout.as_slice(),
            *input,
            "input {i}: round-trip mismatch"
        );
        eprintln!(
            "input {i}: {} bytes -> {} bytes -> {} bytes (OK)",
            input.len(),
            compressed.len(),
            out.stdout.len()
        );
    }
}
