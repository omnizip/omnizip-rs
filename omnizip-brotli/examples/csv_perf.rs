//! Larger CSV benchmark for perf tuning.

use omnizip_brotli::from_spec_encoder::compress_with_quality;
use std::time::Instant;

fn csv_20mb() -> Vec<u8> {
    // Mimic LimniFS csv-synthetic: column-aligned text with high
    // redundancy (column values repeat across rows).
    let mut data = Vec::with_capacity(20 * 1024 * 1024);
    while data.len() < 20 * 1024 * 1024 {
        for i in 0..1000 {
            data.extend_from_slice(
                format!(
                    "id_{i},name_{i},city_{i},country_{i},value_{i},score_{i},tag_{i},group_{i}\n"
                )
                .as_bytes(),
            );
        }
    }
    data.truncate(20 * 1024 * 1024);
    data
}

fn main() {
    let input = csv_20mb();
    println!(
        "Input: {} bytes ({} MiB)",
        input.len(),
        input.len() / (1024 * 1024)
    );

    for &q in &[2, 5, 8, 11] {
        let t = Instant::now();
        let out = compress_with_quality(&input, q);
        let elapsed = t.elapsed();
        let mbps = (input.len() as f64 / 1e6) / elapsed.as_secs_f64().max(1e-9);
        println!(
            "Q{:<3} {} bytes ({:5.2}%) in {:6.3}s ({:6.1} MB/s)",
            q,
            out.len(),
            out.len() as f64 * 100.0 / input.len() as f64,
            elapsed.as_secs_f64(),
            mbps,
        );
    }
}
