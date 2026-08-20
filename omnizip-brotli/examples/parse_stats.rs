//! Dump command-stream statistics to diagnose parse efficiency.

use omnizip_brotli::from_spec_encoder::_parse_input_with_offset_diag;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/csv-100kb.csv".to_string());
    let input = std::fs::read(&path).expect("read");
    let n = input.len();

    for &q in &[5, 11] {
        use omnizip_codecs::{HashChainConfig, HashChainMatchFinder};
        let cfg = HashChainConfig {
            dict_size: 16 * 1024 * 1024,
            min_match: 4,
            hash_bytes: 4,
            max_chain_length: if q >= 11 { 1024 } else { 64 },
            nice_match: if q >= 11 { 4096 } else { 256 },
            hash_log: 18,
            max_match_length: 271,
        };
        let mut mf = HashChainMatchFinder::new(&input, cfg);
        let cmds = _parse_input_with_offset_diag(&input, &mut mf, 0, q, false);

        let n_cmds = cmds.len();
        let copy_cmds: usize = cmds.iter().filter(|c| c.copy_len > 0).count();
        let total_insert: u64 = cmds.iter().map(|c| c.insert_len as u64).sum();
        let total_copy: u64 = cmds
            .iter()
            .filter(|c| c.copy_len > 0)
            .map(|c| c.copy_len as u64)
            .sum::<u64>();
        let max_copy = cmds.iter().map(|c| c.copy_len).max().unwrap_or(0);
        let avg_copy = if copy_cmds > 0 {
            total_copy as f64 / copy_cmds as f64
        } else {
            0.0
        };
        let avg_insert = if n_cmds > 0 {
            total_insert as f64 / n_cmds as f64
        } else {
            0.0
        };
        let copy_pct = total_copy as f64 * 100.0 / n as f64;

        // Distance histogram buckets.
        let mut dist_hist = std::collections::BTreeMap::new();
        for c in &cmds {
            if c.copy_len > 0 {
                let b = match c.distance {
                    0..=4 => "1-4",
                    5..=16 => "5-16",
                    17..=64 => "17-64",
                    65..=256 => "65-256",
                    257..=1024 => "257-1k",
                    1025..=8192 => "1k-8k",
                    8193..=65536 => "8k-64k",
                    _ => ">64k",
                };
                *dist_hist.entry(b).or_insert(0u32) += 1;
            }
        }

        println!("--- q{q} on {path} ({n} bytes) ---");
        println!("commands: {n_cmds} (copy: {copy_cmds}), avg_insert: {avg_insert:.2}, avg_copy: {avg_copy:.2}, max_copy: {max_copy}");
        println!(
            "coverage: {copy_pct:.1}% copied, {} literals inserted ({:.1}% of input)",
            total_insert,
            total_insert as f64 * 100.0 / n as f64
        );
        println!("distances: {:?}", dist_hist);
        // Estimated bits: rough model
        let est_bits: f64 = cmds
            .iter()
            .map(|c| {
                let mut b = 7.0; // cmd symbol
                b += c.insert_len as f64 * 4.0;
                if c.copy_len > 0 {
                    b += (c.distance as f64).log2().min(22.0);
                }
                b
            })
            .sum();
        println!(
            "est bits: {est_bits:.0} = {:.1}% ratio",
            est_bits / 8.0 / n as f64 * 100.0
        );
        // Trace 60 copy commands starting from the 3000th.
        let mut shown = 0;
        let mut skipped = 0;
        for c in &cmds {
            if c.copy_len > 0 {
                skipped += 1;
                if skipped < 3000 {
                    continue;
                }
                println!(
                    "  cmd: ins={} copy={} dist={}",
                    c.insert_len, c.copy_len, c.distance
                );
                shown += 1;
                if shown >= 40 {
                    break;
                }
            }
        }
        println!();
    }
}
