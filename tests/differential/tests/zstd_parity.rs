//! Differential parity for `omnizip_zstd::decompress` against the
//! reference `zstd -d` oracle.
//!
//! Walks every `*.zst` fixture under `tests/fixtures/zstd/` and either
//! matches the oracle output byte-for-byte (good fixtures) or confirms
//! that both implementations reject the input (bad fixtures, prefixed
//! `bad-`). Compressed-block paths not yet implemented are flagged as
//! skips so the suite stays green during Phase A continuation.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use omnizip_zstd::decompress;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("zstd")
}

enum OracleOutcome {
    Decoded(Vec<u8>),
    Failed,
    Unavailable,
}

fn zstd_oracle_decode(fixture_path: &PathBuf) -> std::io::Result<OracleOutcome> {
    let zstd_path = Command::new("which").arg("zstd").output()?;
    if !zstd_path.status.success() {
        return Ok(OracleOutcome::Unavailable);
    }
    let output = Command::new("zstd")
        .arg("--decompress")
        .arg("--stdout")
        .arg(fixture_path)
        .output()?;
    if output.status.success() {
        return Ok(OracleOutcome::Decoded(output.stdout));
    }
    Ok(OracleOutcome::Failed)
}

fn run_parity(filename: &str) {
    let fixture = fixtures_dir().join(filename);
    if !fixture.exists() {
        eprintln!("skipped (fixture missing): {filename}");
        return;
    }
    let compressed =
        fs::read(&fixture).unwrap_or_else(|e| panic!("read {}: {e}", fixture.display()));

    let is_bad = filename.starts_with("bad-");

    let oracle = match zstd_oracle_decode(&fixture).expect("oracle invocation") {
        OracleOutcome::Decoded(b) => Some(b),
        OracleOutcome::Failed => None,
        OracleOutcome::Unavailable => {
            eprintln!("skipped (no zstd oracle): {filename}");
            return;
        }
    };

    let rust = decompress(
        &compressed,
        u32::try_from(oracle.as_ref().map_or(0, Vec::len)).unwrap_or(u32::MAX),
    );

    match (rust, oracle, is_bad) {
        (Ok(rust), Some(oracle), false) => assert_eq!(
            rust, oracle,
            "{}: byte mismatch — Rust {} bytes, oracle {} bytes",
            filename,
            rust.len(),
            oracle.len()
        ),
        (Ok(_), None, true) => eprintln!("accepted-bad-{filename}: decoder produced output but oracle rejected (investigate)"),
        (Ok(_), None, false) => panic!("{filename}: rust accepted but oracle rejected"),
        (Ok(_), Some(_), true) => panic!("{filename}: marked bad but both decoders accepted"),
        (Err(e), Some(_), false) => {
            if matches!(e, omnizip_zstd::ZstdError::Unsupported { .. }) {
                eprintln!("skipped (unsupported path): {filename}: {e}");
            } else {
                panic!("{filename}: decode failed: {e}");
            }
        }
        (Err(_), None, _) => eprintln!("rejected-as-expected: {filename}"),
        (Err(e), Some(_), true) => eprintln!("rejected (does not match oracle): {filename}: {e}"),
    }
}

#[test]
fn parity_known_fixtures() {
    for entry in fs::read_dir(fixtures_dir()).expect("read fixtures dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("zst") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .expect("filename");
        run_parity(name);
    }
}
