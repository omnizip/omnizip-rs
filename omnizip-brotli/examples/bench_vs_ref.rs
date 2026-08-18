//! Benchmark: omnizip-brotli from_spec_encoder vs the reference `brotli`
//! CLI — size AND encode time at every quality level, with reference
//! decode validation of our output.
//!
//! Usage: bench_vs_ref [input-file] [sizes...]
//! Defaults to /tmp/csv-synthetic.csv at 1MB and 4MB slices.

use std::io::Write;
use std::process::{Command, Stdio};

fn ref_compress(data: &[u8], q: i32) -> Option<(Vec<u8>, f64)> {
    let t = std::time::Instant::now();
    let mut child = Command::new("brotli")
        .args(["-q", &q.to_string(), "-c"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.as_mut()?.write_all(data).ok()?;
    let out = child.wait_with_output().ok()?;
    Some((out.stdout, t.elapsed().as_secs_f64()))
}

fn ref_decompress_ok(data: &[u8], expected: &[u8]) -> bool {
    let mut child = match Command::new("brotli")
        .args(["-d", "-c"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    if child.stdin.as_mut().unwrap().write_all(data).is_err() {
        return false;
    }
    match child.wait_with_output() {
        Ok(o) => o.stdout == expected,
        Err(_) => false,
    }
}

fn run(label: &str, data: &[u8], levels: &[i32]) {
    println!("\n=== {label}: {} bytes ===", data.len());
    println!(
        "{:>4} | {:>15} | {:>15} | {:>7} {:>7} | {:>6}",
        "Q", "ours size (ratio)", "ref size (ratio)", "ours_s", "ref_s", "verify"
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
        let ok = ref_decompress_ok(&ours, data);
        println!(
            "{:>4} | {:>9}B ({:4.2}%) | {:>9}B ({:4.2}%) | {:>7.2} {:>7.2} | {}",
            q,
            ours.len(),
            our_ratio,
            refc.len(),
            ref_ratio,
            our_time,
            ref_time,
            if ok { "REF-OK" } else { "REF-FAIL" }
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
