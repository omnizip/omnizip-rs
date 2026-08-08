//! Verify our ZSTD encoder output is decodable by `zstd -d` (C reference).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use std::process::Command;
use tempfile::tempdir;

use omnizip_zstd::{compress, ZstdError, ZstdLevel};

fn zstd_decode_oracle(compressed: &[u8]) -> Result<Vec<u8>, String> {
    let dir = tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let in_path = dir.path().join("input.zst");
    let out_path = dir.path().join("output");
    std::fs::write(&in_path, compressed).map_err(|e| format!("write: {e}"))?;

    let output = Command::new("zstd")
        .arg("--decompress")
        .arg("-f")
        .arg("-o")
        .arg(&out_path)
        .arg(&in_path)
        .output()
        .map_err(|e| format!("spawn zstd: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "zstd -d failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    std::fs::read(&out_path).map_err(|e| format!("read output: {e}"))
}

fn check_roundtrip(name: &str, input: &[u8], level: ZstdLevel) {
    let compressed = match compress(input, level) {
        Ok(c) => c,
        Err(ZstdError::LevelUnavailable { .. }) => return,
        Err(e) => panic!("zstd encode {name} at {level}: {e:?}"),
    };

    match zstd_decode_oracle(&compressed) {
        Ok(decoded) => {
            assert_eq!(
                decoded, input,
                "zstd -d output mismatch for {name} at {level}"
            );
        }
        Err(e) => {
            panic!("zstd -d rejected our output for {name} at {level}: {e}");
        }
    }
}

#[test]
fn encoder_output_decodes_via_c_reference() {
    let inputs: Vec<(&str, Vec<u8>)> = vec![
        ("short", b"hello world".to_vec()),
        ("repetitive", b"abcabcabcabc".repeat(50)),
        (
            "text",
            b"The quick brown fox jumps over the lazy dog. ".repeat(100),
        ),
        ("zeros", vec![0u8; 10_000]),
        ("binary", (0..5000).map(|i| (i % 251) as u8).collect()),
        ("large_text", b"the brown fox ".repeat(10_000)),
    ];

    for (name, input) in &inputs {
        for level in [
            ZstdLevel::Fastest,
            ZstdLevel::Fast,
            ZstdLevel::Default,
            ZstdLevel::Better,
            ZstdLevel::Best,
        ] {
            check_roundtrip(name, input, level);
        }
    }
}
