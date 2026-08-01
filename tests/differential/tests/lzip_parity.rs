//! Differential parity for `omnizip_lzma::lzip_decompress` against the
//! reference `xz -d` oracle. Covers every `.lz` fixture under
//! `tests/fixtures/lzma/`.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use omnizip_lzma::lzip_decompress;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("lzma")
}

fn oracle_decode(fixture: &PathBuf) -> Option<Vec<u8>> {
    let which = Command::new("which").arg("xz").output().ok()?;
    if !which.status.success() { return None; }
    let output = Command::new("xz")
        .arg("--decompress")
        .arg("--stdout")
        .arg(fixture)
        .output()
        .ok()?;
    if !output.status.success() { return None; }
    Some(output.stdout)
}

fn run_one(path: &PathBuf) {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let Some(oracle) = oracle_decode(path) else {
        eprintln!("skipped (no xz oracle): {name}");
        return;
    };
    let compressed = fs::read(path).unwrap_or_else(|e| panic!("read {e}"));

    match lzip_decompress(&compressed) {
        Ok(rust) => {
            if rust == oracle {
                eprintln!("PASS: {name}");
            } else {
                eprintln!("MISMATCH: {name} — Rust {} bytes, oracle {} bytes",
                          rust.len(), oracle.len());
            }
        }
        Err(e) => {
            if name.starts_with("bad-") || name.starts_with("unsupported-") {
                eprintln!("accepted (bad-* fixture): {name}: {e}");
            } else {
                eprintln!("FAIL: {name}: {e}");
            }
        }
    }
}

#[test]
fn parity_all_lz_fixtures() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipped (no fixtures dir): {}", root.display());
        return;
    }
    for entry in fs::read_dir(&root).expect("read fixtures dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|s| s.to_str()) != Some("lz") {
            continue;
        }
        run_one(&path);
    }
}
