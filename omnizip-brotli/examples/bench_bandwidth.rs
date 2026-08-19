//! Bandwidth benchmark vs the reference `brotli` CLI: MB/s of
//! uncompressed data for encode AND decode, per quality level.
//!
//! Usage: bench_bandwidth [input-file] [sizes...]
//! LEVELS=1,5,9,11 selects qualities (default 1,5,9,11).

use std::process::{Command, Stdio};

const MB: f64 = 1e6;

fn ref_run(
    args: &[&str],
    infile: &std::path::PathBuf,
    expected: Option<&[u8]>,
) -> Option<(Vec<u8>, f64)> {
    let t = std::time::Instant::now();
    let out = Command::new("brotli")
        .args(args)
        .arg(infile)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let secs = t.elapsed().as_secs_f64();
    if !out.status.success() {
        return None;
    }
    if let Some(exp) = expected {
        if out.stdout != exp {
            return None;
        }
    }
    Some((out.stdout, secs))
}

fn run(label: &str, data: &[u8], levels: &[i32]) {
    let n_mb = data.len() as f64 / MB;
    let tmp = std::env::temp_dir().join(format!("bw_in_{}", std::process::id()));
    std::fs::write(&tmp, data).unwrap();

    println!("\n=== {label}: {} ({n_mb:.1} MB) ===", data.len());
    println!(
        "{:>4} | {:>9} {:>9} | {:>8} {:>8} {:>5} | {:>8} {:>8} {:>5} | {:>6}",
        "Q", "size_o", "size_r", "enc_o", "enc_r", "x", "dec_o", "dec_r", "x", "verify"
    );
    for &q in levels {
        let t = std::time::Instant::now();
        let ours = omnizip_brotli::from_spec_encoder::compress_with_quality(data, q);
        let enc_o = n_mb / t.elapsed().as_secs_f64();

        let (refc, enc_secs) = match ref_run(&["-q", &q.to_string(), "-c"], &tmp, None) {
            Some(r) => r,
            None => {
                println!("{q:>4} | reference CLI unavailable");
                continue;
            }
        };
        let enc_r = n_mb / enc_secs;

        // Decode bandwidth: ours on our output, ref on its own output.
        let t = std::time::Instant::now();
        let rt = omnizip_brotli::decoder::decode(&ours).ok();
        let dec_o = n_mb / t.elapsed().as_secs_f64();
        let rt_ok = rt.as_ref().is_some_and(|r| r == data);

        let br_path = std::env::temp_dir().join(format!("bw_br_{}", std::process::id()));
        std::fs::write(&br_path, &refc).unwrap();
        let (_, dec_secs) =
            ref_run(&["-d", "-c"], &br_path, Some(data)).unwrap_or((vec![], f64::NAN));
        let dec_r = n_mb / dec_secs;
        let ref_rt_ok = !dec_secs.is_nan();

        // Interop: reference accepts our stream.
        std::fs::write(&br_path, &ours).unwrap();
        let interop = ref_run(&["-d", "-c"], &br_path, Some(data)).is_some();
        let _ = std::fs::remove_file(&br_path);

        let verify = if rt_ok && ref_rt_ok && interop {
            "OK"
        } else {
            "FAIL"
        };
        println!(
            "{:>4} | {:>9} {:>9} | {:>8.1} {:>8.1} {:>5.1} | {:>8.1} {:>8.1} {:>5.1} | {:>6}",
            q,
            ours.len(),
            refc.len(),
            enc_o,
            enc_r,
            enc_r / enc_o,
            dec_o,
            dec_r,
            dec_r / dec_o,
            verify
        );
    }
    let _ = std::fs::remove_file(&tmp);
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
        vec![1 << 20]
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
