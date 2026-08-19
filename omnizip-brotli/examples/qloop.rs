//! Compress a file N times at a quality — for profiling loops.
//! Usage: qloop [path] [q] [iters]
fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/csv1m.bin".into());
    let q: i32 = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let iters: usize = std::env::args()
        .nth(3)
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let data = std::fs::read(&path).expect("read");
    let mut n = 0usize;
    for _ in 0..iters {
        let out = omnizip_brotli::from_spec_encoder::compress_with_quality(&data, q);
        n = out.len();
    }
    println!("q{q}: {n} bytes x {iters}");
}
