//! Timed decode loop for profiling: decode a .br file N times, report MB/s.
//! Usage: decode_prof [path] [iters]

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/fits_out.br".to_string());
    let iters: usize = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let data = std::fs::read(&path).expect("read");
    let mut out_len = 0usize;
    // Warm up once.
    let warm = omnizip_brotli::decoder::decode(&data).expect("decode");
    out_len = warm.len();
    let t = std::time::Instant::now();
    for _ in 0..iters {
        let d = omnizip_brotli::decoder::decode(&data).expect("decode");
        out_len = d.len();
    }
    let el = t.elapsed().as_secs_f64();
    println!(
        "{path}: {out_len} bytes x {iters} iters: {el:.3}s total, {:.1} MB/s output",
        (out_len as f64 * iters as f64) / el / 1e6
    );
}
