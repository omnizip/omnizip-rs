use omnizip_brotli::from_spec_encoder::_fast_log2_diag;

fn main() {
    // 1. fast_log2 accuracy across scales
    let mut worst = 0.0f64;
    for &v in &[
        1u32, 2, 3, 4, 7, 9, 16, 31, 100, 255, 256, 1000, 4095, 65535, 65536, 65537, 100_000,
        524_288, 600_000, 1_000_000, 2_097_152,
    ] {
        let approx = _fast_log2_diag(v);
        let exact = (f64::from(v)).log2();
        let err = (approx - exact).abs();
        if err > worst {
            worst = err;
        }
        if err > 0.001 {
            println!("BAD v={v} approx={approx:.6} exact={exact:.6}");
        }
    }
    println!("fast_log2 worst err: {worst:.2e}");

    // sweep: find fast_log2 misbehavior across 1..70000
    let mut bad = 0u32;
    let mut worst2 = 0.0f64;
    let mut worst_v = 0u32;
    for v in 1u32..70_000 {
        let a = _fast_log2_diag(v);
        let e = f64::from(v).log2();
        let d = (a - e).abs();
        if d > worst2 {
            worst2 = d;
            worst_v = v;
        }
        if d > 0.001 {
            bad += 1;
        }
    }
    println!("sweep: bad={bad} worst={worst2:.4e} at v={worst_v}");

    // 2. incremental entropy vs batch on a synthetic histogram merge
    let mut rng: u64 = 0x1234_5678_9abc_def0;
    let mut rnd = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let m = 40usize;
    let step = 64usize;
    let mut seg_hist = vec![[0u32; 704]; m];
    for i in 0..m {
        for _ in 0..step {
            let s = (rnd() % 50) as usize;
            seg_hist[i][s] += 1;
        }
    }
    let cuts: Vec<usize> = (0..=m).map(|i| i * step).collect();
    let mut hist = vec![0u32; 704];
    let mut t: u64 = 0;
    let mut bits = 0.0f64;
    let mut maxdiff = 0.0f64;
    for i in 0..m {
        for s in 0..704 {
            let c = u64::from(seg_hist[i][s]);
            if c == 0 {
                continue;
            }
            let f0 = u64::from(hist[s]);
            let f1 = f0 + c;
            hist[s] = f1 as u32;
            let t1 = t + c;
            bits += f64::from(t1 as u32) * _fast_log2_diag(t1 as u32)
                - f64::from(t as u32) * _fast_log2_diag(t as u32)
                + f64::from(f0 as u32) * _fast_log2_diag(f0 as u32)
                - f64::from(f1 as u32) * _fast_log2_diag(f1 as u32);
            t = t1;
        }
        // batch formula
        let total: u64 = hist.iter().map(|&x| u64::from(x)).sum();
        let mut bb = total as f64 * (total as f64).log2();
        for &f in hist.iter() {
            if f > 0 {
                bb -= f as f64 * (f as f64).log2();
            }
        }
        let d = (bits - bb).abs();
        if d > maxdiff {
            maxdiff = d;
        }
        if i < 5 || d > 100.0 {
            println!("seg {i}: inc={bits:.4} batch={bb:.4} diff={d:.4} t={t}");
        }
    }
    println!("incremental-vs-batch max diff: {maxdiff:.2e}");
}
