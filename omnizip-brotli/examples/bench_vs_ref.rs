//! Benchmark: omnizip-brotli from_spec_encoder vs the reference `brotli`
//! CLI — size AND encode time at every quality level, with reference
//! decode validation of our output.
//!
//! Usage: bench_vs_ref [input-file] [sizes...]
//! Defaults to /tmp/csv-synthetic.csv at 1MB and 4MB slices.

use std::io::Write;
use std::process::{Command, Stdio};

fn ref_compress(data: &[u8], q: i32) -> Option<(Vec<u8>, f64)> {
    let infile = std::env::temp_dir().join(format!("bench_ref_in_{}", std::process::id()));
    std::fs::write(&infile, data).ok()?;
    let t = std::time::Instant::now();
    // Feed via file arg + `.output()`: piping 21MB into stdin while the
    // child fills its stdout pipe deadlocks both processes.
    let out = Command::new("brotli")
        .args(["-q", &q.to_string(), "-c"])
        .arg(&infile)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let secs = t.elapsed().as_secs_f64();
    let _ = std::fs::remove_file(&infile);
    Some((out.stdout, secs))
}

fn ref_decompress_ok(data: &[u8], expected: &[u8]) -> bool {
    let infile = std::env::temp_dir().join(format!("bench_ref_dec_{}", std::process::id()));
    if std::fs::write(&infile, data).is_err() {
        return false;
    }
    let out = Command::new("brotli")
        .args(["-d", "-c"])
        .arg(&infile)
        .stderr(Stdio::null())
        .output();
    let _ = std::fs::remove_file(&infile);
    matches!(&out, Ok(o) if o.status.success() && o.stdout == expected)
}

fn run(label: &str, data: &[u8], levels: &[i32]) {
    println!("\n=== {label}: {} bytes ===", data.len());
    println!(
        "{:>4} | {:>15} | {:>15} | {:>7} {:>7} | {:>6} {:>6} | {:>6}",
        "Q",
        "ours size (ratio)",
        "ref size (ratio)",
        "enc_s",
        "ref_s",
        "dec_s",
        "ref_dec",
        "verify"
    );
    for &q in levels {
        let t = std::time::Instant::now();
        let ours = omnizip_brotli::from_spec_encoder::compress_with_quality(data, q);
        let our_time = t.elapsed().as_secs_f64();
        let our_ratio = ours.len() as f64 * 100.0 / data.len() as f64;
        let (refc, ref_time) = match ref_compress(data, q) {
            Some(r) => r,
            None => {
                println!("{q:>4} | reference CLI unavailable");
                continue;
            }
        };
        let ref_ratio = refc.len() as f64 * 100.0 / data.len() as f64;

        // Our decoder on our output.
        let t = std::time::Instant::now();
        let roundtrip = omnizip_brotli::decoder::decode(&ours).ok();
        let our_dec_time = t.elapsed().as_secs_f64();
        let rt_ok = roundtrip.as_ref().is_some_and(|rt| rt == data);

        // Reference decoder on reference output (its own decode perf).
        let t = std::time::Instant::now();
        let ref_rt_ok = ref_decompress_ok(&refc, data);
        let ref_dec_time = t.elapsed().as_secs_f64();

        // Interop: reference decoder must accept our output.
        let interop_ok = ref_decompress_ok(&ours, data);
        let verify = if rt_ok && ref_rt_ok && interop_ok {
            "REF-OK"
        } else {
            "REF-FAIL"
        };
        println!(
            "{:>4} | {:>9}B ({:4.2}%) | {:>9}B ({:4.2}%) | {:>7.2} {:>7.2} | {:>6.2} {:>6.2} | {}",
            q,
            ours.len(),
            our_ratio,
            refc.len(),
            ref_ratio,
            our_time,
            ref_time,
            our_dec_time,
            ref_dec_time,
            verify
        );
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/csv-synthetic.csv".to_string());
    let full = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let sizes: Vec<usize> = std::env::args()
        .skip(2)
        .filter_map(|a| a.parse().ok())
        .collect();
    let sizes = if sizes.is_empty() {
        vec![1 << 20, 4 << 20]
    } else {
        sizes
    };
    let levels: Vec<i32> = std::env::var("LEVELS")
        .ok()
        .map(|v| v.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_else(|| vec![1, 5, 9, 11]);
    for sz in sizes {
        let n = sz.min(full.len());
        run(
            &format!("{} [0..{}]", path.rsplit('/').next().unwrap_or(&path), n),
            &full[..n],
            &levels,
        );
    }
}
