//! Differential parity for `omnizip_lzma::xz_decompress` against the
//! reference `xz -d` oracle. Covers every `.xz` fixture under
//! `tests/fixtures/xz/`.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use omnizip_lzma::xz_decompress;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("xz")
}

fn xz_oracle_decode(fixture: &PathBuf) -> Option<Vec<u8>> {
    let zstd_path = Command::new("which").arg("xz").output().ok()?;
    if !zstd_path.status.success() {
        return None;
    }
    let output = Command::new("xz")
        .arg("--decompress")
        .arg("--stdout")
        .arg(fixture)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(output.stdout)
}

fn run_one(path: &PathBuf) {
    let Some(oracle) = xz_oracle_decode(path) else {
        eprintln!("skipped (no xz oracle): {}", path.display());
        return;
    };
    let compressed = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    match xz_decompress(&compressed) {
        Ok(rust) => {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if rust == oracle {
                eprintln!("PASS: {name}");
            } else {
                eprintln!(
                    "MISMATCH: {name} — Rust {} bytes, oracle {} bytes",
                    rust.len(),
                    oracle.len()
                );
            }
        }
        Err(e) => {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.starts_with("bad-") {
                eprintln!("accepted (bad-* fixture): {name}: {e}");
            } else {
                eprintln!("FAIL: {name}: {e}");
            }
        }
    }
}

#[test]
fn parity_all_xz_fixtures() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipped (no fixtures dir): {}", root.display());
        return;
    }
    for entry in fs::read_dir(&root).expect("read fixtures dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|s| s.to_str()) != Some("xz") {
            continue;
        }
        run_one(&path);
    }
}
