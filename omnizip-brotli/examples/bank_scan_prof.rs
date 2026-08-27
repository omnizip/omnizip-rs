//! Profile harness for the q2 bank-scan path: loops compress_with_quality
//! on the 2 MiB regression-CSV fixture so `sample`/`perf` can catch it.
//!
//! Usage: bank_scan_prof [iterations] [quality] [dump]
//! Prints per-iteration size + a total, and keeps the process alive for
//! the duration (sample <pid> needs a live target). `dump` writes the
//! fixture + compressed bytes to /tmp for cross-checking.

fn csv_2mib() -> Vec<u8> {
    // Mirrors tests/benchmarks/regression.rs csv_data at 2048 KB.
    let row = b"id,name,city,country,population,area_code,latitude,longitude,status\n";
    let size_kb = 2048usize;
    let rows = size_kb * 1024 / row.len();
    let mut data = Vec::with_capacity(size_kb * 1024);
    for i in 0..rows {
        data.extend_from_slice(
            format!(
                "{i},user_{i},city_{},cc,{},{},{}.{},{i}\n",
                i % 1000,
                i % 1000000,
                (i % 360) as i32 - 180,
                i / 1000,
                i % 1000,
            )
            .as_bytes(),
        );
    }
    data
}

fn main() {
    let iters: usize = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let quality: i32 = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let input = csv_2mib();
    if std::env::args().nth(3).as_deref() == Some("dump") {
        std::fs::write("/tmp/csv2m_rust.bin", &input).unwrap();
        let out = omnizip_brotli::from_spec_encoder::compress_with_quality(&input, quality);
        std::fs::write("/tmp/ours_q2.br", &out).unwrap();
        eprintln!(
            "dumped {} fixture bytes + {} compressed bytes",
            input.len(),
            out.len()
        );
        return;
    }
    eprintln!("fixture: {} bytes, q{quality}, iters {iters}", input.len());
    let mut sizes = Vec::new();
    let t0 = std::time::Instant::now();
    for it in 0..iters {
        let t = std::time::Instant::now();
        let out = omnizip_brotli::from_spec_encoder::compress_with_quality(&input, quality);
        sizes.push(out.len());
        eprintln!(
            "iter {it}: {} bytes in {:.4}s",
            out.len(),
            t.elapsed().as_secs_f64()
        );
    }
    eprintln!(
        "total {:.4}s, sizes identical: {}",
        t0.elapsed().as_secs_f64(),
        sizes.windows(2).all(|w| w[0] == w[1])
    );
}
