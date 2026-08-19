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
    if std::env::var("BROTLI_HT_STATS").is_ok() {
        let (n, smp, cx, ns) = omnizip_brotli::decoder::_ht_stats();
        println!(
            "HT tables={n} simple={smp} complex={cx} total={}us avg={}ns/table",
            ns / 1000,
            if n > 0 { ns / n } else { 0 }
        );
    }
    if std::env::var("BROTLI_HT_STATS").is_ok() {
        let (n, l, b) = omnizip_brotli::decoder::_cl_stats();
        println!(
            "CL loops={n} read={}us avg={}ns build={}us avg={}ns",
            l / 1000,
            if n > 0 { l / n } else { 0 },
            b / 1000,
            if n > 0 { b / n } else { 0 }
        );
    }
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

#[allow(clippy::tuple_array_conversions)]
fn _unused() {}
