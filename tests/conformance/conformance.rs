//! Codec conformance test suite (TODO 271).
//!
//! Walks each codec's official test vector corpus (when downloaded)
//! and verifies our decoder accepts every file. Catches wire-format
//! bugs that round-trip tests miss (because round-trip uses our own
//! encoder's output).
//!
//! ## Setup
//!
//! Test vectors are NOT vendored (too large). Run
//! `tests/fixtures/corpora/setup.sh` first to download.
//!
//! ## Running
//!
//! ```bash
//! # If corpora are downloaded, this runs the full conformance suite.
//! # If not, the test is a no-op (returns early).
//! cargo test --test conformance --release
//! ```

use omnizip_codecs::Codec;
use std::path::PathBuf;
use walkdir::WalkDir;

const CONFORMANCE_ROOT: &str = "tests/fixtures/conformance";

/// True if a directory exists (i.e., test vectors have been downloaded).
fn corpus_available(name: &str) -> bool {
    let path: PathBuf = [CONFORMANCE_ROOT, name].iter().collect();
    path.is_dir()
        && path
            .read_dir()
            .map(|mut it| it.next().is_some())
            .unwrap_or(false)
}

/// Walk every file under `dir`, call `f` with its bytes.
fn for_each_file<F: FnMut(&std::path::Path, &[u8])>(dir: &str, mut f: F) {
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        f(path, &bytes);
    }
}

/// Generic conformance check: every `.br` file under the Brotli
/// corpus dir must decode successfully via our decoder.
#[test]
fn brotli_accepts_all_official_test_vectors() {
    if !corpus_available("brotli") {
        eprintln!("[conformance] skipping brotli: corpus not downloaded");
        eprintln!("[conformance] run: tests/fixtures/corpora/setup.sh brotli");
        return;
    }
    let codec = omnizip_brotli::BrotliCodec::new();
    let mut tested = 0u32;
    let mut failed = 0u32;
    let dir: PathBuf = [CONFORMANCE_ROOT, "brotli"].iter().collect();
    for_each_file(dir.to_str().expect("utf8"), |_path, bytes| {
        tested += 1;
        // expected_len unknown; use u32::MAX and let decoder figure it out.
        if codec.decompress(bytes, u32::MAX).is_err() {
            failed += 1;
        }
    });
    assert!(
        failed == 0,
        "{failed}/{tested} brotli vectors failed to decode"
    );
    eprintln!("[conformance] brotli: {tested} vectors all decoded OK");
}

/// Same for ZSTD.
#[test]
fn zstd_accepts_all_official_test_vectors() {
    if !corpus_available("zstd") {
        eprintln!("[conformance] skipping zstd: corpus not downloaded");
        return;
    }
    let codec = omnizip_zstd::ZstdCodec::new();
    let mut tested = 0u32;
    let mut failed = 0u32;
    let dir: PathBuf = [CONFORMANCE_ROOT, "zstd"].iter().collect();
    for_each_file(dir.to_str().expect("utf8"), |_path, bytes| {
        tested += 1;
        if codec.decompress(bytes, u32::MAX).is_err() {
            failed += 1;
        }
    });
    assert!(failed == 0, "{failed}/{tested} zstd vectors failed");
    eprintln!("[conformance] zstd: {tested} vectors all decoded OK");
}

/// Same for LZMA / xz.
#[test]
fn lzma_accepts_all_official_test_vectors() {
    if !corpus_available("xz") {
        eprintln!("[conformance] skipping lzma: corpus not downloaded");
        return;
    }
    let codec = omnizip_lzma::LzmaCodec::new();
    let mut tested = 0u32;
    let mut failed = 0u32;
    let dir: PathBuf = [CONFORMANCE_ROOT, "xz"].iter().collect();
    for_each_file(dir.to_str().expect("utf8"), |_path, bytes| {
        tested += 1;
        if codec.decompress(bytes, u32::MAX).is_err() {
            failed += 1;
        }
    });
    assert!(failed == 0, "{failed}/{tested} lzma vectors failed");
    eprintln!("[conformance] lzma: {tested} vectors all decoded OK");
}

/// Same for LZ4.
#[test]
fn lz4_accepts_all_official_test_vectors() {
    if !corpus_available("lz4") {
        eprintln!("[conformance] skipping lz4: corpus not downloaded");
        return;
    }
    let codec = omnizip_lz4::Lz4FastCodec;
    let mut tested = 0u32;
    let mut failed = 0u32;
    let dir: PathBuf = [CONFORMANCE_ROOT, "lz4"].iter().collect();
    for_each_file(dir.to_str().expect("utf8"), |_path, bytes| {
        tested += 1;
        if codec.decompress(bytes, u32::MAX).is_err() {
            failed += 1;
        }
    });
    assert!(failed == 0, "{failed}/{tested} lz4 vectors failed");
    eprintln!("[conformance] lz4: {tested} vectors all decoded OK");
}
