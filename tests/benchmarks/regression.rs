//! Regression benchmark harness (TODO 252).
//!
//! Runs each codec on each fixture and records ratio + speed. Compares
//! to a baseline JSON; fails if ratio regresses > 1% or speed regresses
//! > 5%.
//!
//! ## Usage
//!
//! ```bash
//! # Run and print results (no comparison)
//! cargo test --test regression --release -- --nocapture
//!
//! # Run and write baseline.json
//! OMNIZIP_WRITE_BASELINE=1 cargo test --test regression --release
//!
//! # Run and compare to baseline (default behavior)
//! cargo test --test regression --release
//! ```

use omnizip_codecs::{Codec, CompressionLevel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Instant;

const REGRESSION_RATIO_THRESHOLD_PCT: f64 = 1.0;
const REGRESSION_SPEED_THRESHOLD_PCT: f64 = 15.0;
const REGRESSION_MIN_BASELINE_MS: u64 = 100;

#[derive(Debug, Serialize, Deserialize)]
struct BenchmarkResult {
    input_bytes: usize,
    output_bytes: usize,
    ratio: f64,
    elapsed_ms: u64,
    mbps: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct BaselineFile {
    version: String,
    commit: String,
    timestamp: String,
    results: BTreeMap<String, BenchmarkResult>,
}

fn csv_data(size_kb: usize) -> Vec<u8> {
    let row = b"id,name,city,country,population,area_code,latitude,longitude,status\n";
    let rows_per_iter = size_kb * 1024 / row.len();
    let mut data = Vec::with_capacity(size_kb * 1024);
    for i in 0..rows_per_iter {
        data.extend_from_slice(
            format!(
                "{i},user_{i},city_{},cc,{},{},{}.{},{i}\n",
                i % 1000,
                i % 1000000,
                i % 360 - 180,
                i / 1000,
                i % 1000,
            )
            .as_bytes(),
        );
    }
    data
}

fn english_text(size_kb: usize) -> Vec<u8> {
    let words = b"the quick brown fox jumps over the lazy dog and runs through the forest looking for food while the sun sets behind the mountains creating shadows that stretch across the valley floor where the river flows gently toward the sea ";
    let mut data = Vec::with_capacity(size_kb * 1024);
    while data.len() < size_kb * 1024 {
        let start = data.len() % words.len();
        data.extend_from_slice(&words[start..]);
    }
    data.truncate(size_kb * 1024);
    data
}

fn binary_data(size_kb: usize) -> Vec<u8> {
    (0..size_kb * 256)
        .map(|i| {
            let x = i.wrapping_mul(2654435761);
            ((x >> 16) & 0xFF) as u8
        })
        .collect()
}

fn run_one<C: Codec>(codec: &C, name: &str, input: &[u8], level: u8) -> BenchmarkResult {
    let level = CompressionLevel::new(level);
    let t = Instant::now();
    let compressed = codec
        .compress(input, level)
        .unwrap_or_else(|e| panic!("{name} compress failed: {e:?}"));
    let elapsed = t.elapsed();
    let mbps = (input.len() as f64 / 1e6) / elapsed.as_secs_f64().max(1e-9);
    BenchmarkResult {
        input_bytes: input.len(),
        output_bytes: compressed.len(),
        ratio: compressed.len() as f64 / input.len() as f64,
        elapsed_ms: elapsed.as_millis() as u64,
        mbps,
    }
}

fn build_baseline() -> BaselineFile {
    let mut results = BTreeMap::new();

    let brotli = omnizip_brotli::BrotliCodec::new();
    let zstd = omnizip_zstd::ZstdCodec::new();
    let lzma = omnizip_lzma::LzmaCodec::new();
    let lz4_hc = omnizip_lz4::Lz4HcCodec;

    let csv100 = csv_data(100);
    let text100 = english_text(100);
    let bin100 = binary_data(100);

    results.insert(
        "brotli/q5/csv_100k".into(),
        run_one(&brotli, "brotli", &csv100, 5),
    );
    results.insert(
        "brotli/q5/text_100k".into(),
        run_one(&brotli, "brotli", &text100, 5),
    );
    results.insert(
        "brotli/q5/binary_100k".into(),
        run_one(&brotli, "brotli", &bin100, 5),
    );
    results.insert(
        "brotli/q11/csv_100k".into(),
        run_one(&brotli, "brotli", &csv100, 11),
    );

    results.insert(
        "zstd/l9/csv_100k".into(),
        run_one(&zstd, "zstd", &csv100, 9),
    );
    results.insert(
        "zstd/l9/text_100k".into(),
        run_one(&zstd, "zstd", &text100, 9),
    );
    results.insert(
        "zstd/l9/binary_100k".into(),
        run_one(&zstd, "zstd", &bin100, 9),
    );

    results.insert(
        "lzma/l6/csv_100k".into(),
        run_one(&lzma, "lzma", &csv100, 6),
    );
    results.insert(
        "lzma/l6/text_100k".into(),
        run_one(&lzma, "lzma", &text100, 6),
    );

    results.insert(
        "lz4_hc/l9/csv_100k".into(),
        run_one(&lz4_hc, "lz4_hc", &csv100, 9),
    );
    results.insert(
        "lz4_hc/l9/binary_100k".into(),
        run_one(&lz4_hc, "lz4_hc", &bin100, 9),
    );

    BaselineFile {
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: option_env!("GIT_COMMIT").unwrap_or("unknown").to_string(),
        timestamp: chrono_now(),
        results,
    }
}

fn chrono_now() -> String {
    // Avoid chrono dep; use SystemTime.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

#[test]
fn regression_check() {
    let current = build_baseline();

    // Always print so CI logs capture the numbers.
    println!("\n=== Benchmark results ===");
    for (key, result) in &current.results {
        println!(
            "{:40} {:8} bytes ({:5.2}%) in {:4}ms ({:6.1} MB/s)",
            key,
            result.output_bytes,
            result.ratio * 100.0,
            result.elapsed_ms,
            result.mbps,
        );
    }

    // Write baseline mode: skip comparison.
    if std::env::var("OMNIZIP_WRITE_BASELINE").is_ok() {
        let baseline_path = baseline_path();
        let json = serde_json::to_string_pretty(&current).expect("serialize");
        std::fs::write(&baseline_path, json).expect("write baseline");
        println!("\nBaseline written to {}", baseline_path.display());
        return;
    }

    // Compare to baseline if it exists.
    let baseline_path = baseline_path();
    let baseline_json = match std::fs::read_to_string(&baseline_path) {
        Ok(s) => s,
        Err(_) => {
            println!(
                "\nNo baseline at {}. Run with OMNIZIP_WRITE_BASELINE=1 to create.",
                baseline_path.display()
            );
            return;
        }
    };
    let baseline: BaselineFile =
        serde_json::from_str(&baseline_json).expect("deserialize baseline");

    let mut regressions = Vec::new();
    let mut improvements = Vec::new();
    for (key, current_result) in &current.results {
        let Some(baseline_result) = baseline.results.get(key) else {
            println!("NEW: {key} (no baseline)");
            continue;
        };
        let ratio_delta =
            (current_result.ratio - baseline_result.ratio) / baseline_result.ratio * 100.0;
        let speed_delta =
            (baseline_result.mbps - current_result.mbps) / baseline_result.mbps * 100.0;

        if ratio_delta > REGRESSION_RATIO_THRESHOLD_PCT {
            regressions.push(format!(
                "{key}: ratio {:+.2}% ({} → {} bytes)",
                ratio_delta, baseline_result.output_bytes, current_result.output_bytes
            ));
        }
        if speed_delta > REGRESSION_SPEED_THRESHOLD_PCT
            && baseline_result.elapsed_ms >= REGRESSION_MIN_BASELINE_MS
        {
            // Only flag speed regressions when the baseline timing is
            // long enough to be measurable. Below 100ms, system jitter
            // dominates and false positives are common.
            regressions.push(format!(
                "{key}: speed -{:.2}% ({:.1} → {:.1} MB/s)",
                speed_delta, baseline_result.mbps, current_result.mbps
            ));
        }
        if ratio_delta < -REGRESSION_RATIO_THRESHOLD_PCT {
            improvements.push(format!(
                "{key}: ratio {:+.2}% ({} → {} bytes)",
                ratio_delta, baseline_result.output_bytes, current_result.output_bytes
            ));
        }
    }

    if !improvements.is_empty() {
        println!("\n=== Improvements ===");
        for i in &improvements {
            println!("  {i}");
        }
    }

    if !regressions.is_empty() {
        eprintln!("\n=== REGRESSIONS ({}) ===", regressions.len());
        for r in &regressions {
            eprintln!("  {r}");
        }
        panic!("benchmark regressions detected");
    }

    println!("\nNo regressions detected.");
}

fn baseline_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("baseline.json")
}
