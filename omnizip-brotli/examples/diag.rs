//! Diagnose brotli encoding: dump command/literal stats.

use omnizip_brotli::from_spec_encoder::{Command, _parse_input_with_offset_diag};
use omnizip_codecs::{HashChainConfig, HashChainMatchFinder};

fn cmd_stats(cmds: &[Command], input_len: usize, label: &str) {
    let n_cmds = cmds.len();
    let total_insert: u64 = cmds.iter().map(|c| c.insert_len as u64).sum();
    let total_copy: u64 = cmds.iter().map(|c| c.copy_len as u64).sum();
    let n_matches: usize = cmds.iter().filter(|c| c.copy_len > 0).count();
    let avg_copy = if n_matches > 0 {
        total_copy as f64 / n_matches as f64
    } else {
        0.0
    };
    let avg_insert = if n_cmds > 0 {
        total_insert as f64 / n_cmds as f64
    } else {
        0.0
    };
    let literal_frac = total_insert as f64 * 100.0 / input_len as f64;
    let copy_frac = total_copy as f64 * 100.0 / input_len as f64;

    // Count dict matches: a command is a dict reference if distance > output position.
    // Approximate by checking if distance > cumulative output position.
    let mut dict_matches = 0usize;
    let mut lz77_matches = 0usize;
    let mut output_pos = 0usize;
    let mut rep = [0u32; 4];
    let mut rep_hits = 0usize;
    let mut first_20 = String::new();
    for (i, c) in cmds.iter().enumerate() {
        output_pos += c.insert_len as usize;
        if c.copy_len > 0 {
            if (c.distance as usize) > output_pos {
                dict_matches += 1;
            } else {
                lz77_matches += 1;
            }
            if rep.contains(&c.distance) {
                rep_hits += 1;
            }
            // Update rep ring (most-recent first)
            if c.distance != rep[0] {
                rep[3] = rep[2];
                rep[2] = rep[1];
                rep[1] = rep[0];
                rep[0] = c.distance;
            }
            output_pos += c.copy_len as usize;
            if i < 20 && c.copy_len > 0 {
                first_20.push_str(&format!(
                    "    cmd[{}]: insert={} copy={} dist={}\n",
                    i, c.insert_len, c.copy_len, c.distance
                ));
            }
        }
    }

    println!("{label}: {n_cmds} cmds ({dict_matches} dict, {lz77_matches} lz77)");
    println!(
        "  total_insert={total_insert} ({literal_frac:.1}% of input), avg {avg_insert:.1}B/cmd"
    );
    println!("  total_copy={total_copy} ({copy_frac:.1}% of input), avg {avg_copy:.1}B/match");
    println!(
        "  rep-code-able: {rep_hits}/{lz77_matches} ({:.1}%)",
        rep_hits as f64 * 100.0 / lz77_matches.max(1) as f64
    );
    if !first_20.is_empty() {
        println!("  first matches:\n{first_20}");
    }

    // Match length histogram
    let mut buckets = [0u32; 8];
    for c in cmds {
        if c.copy_len == 0 {
            continue;
        }
        let b = match c.copy_len {
            2..=4 => 0,
            5..=8 => 1,
            9..=16 => 2,
            17..=32 => 3,
            33..=64 => 4,
            65..=128 => 5,
            129..=256 => 6,
            _ => 7,
        };
        buckets[b] += 1;
    }
    println!(
        "  match len: 2-4={}, 5-8={}, 9-16={}, 17-32={}, 33-64={}, 65-128={}, 129-256={}, 256+={}",
        buckets[0],
        buckets[1],
        buckets[2],
        buckets[3],
        buckets[4],
        buckets[5],
        buckets[6],
        buckets[7]
    );

    // Distance histogram (top 10)
    let mut dist_freq: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for c in cmds {
        if c.copy_len > 0 {
            *dist_freq.entry(c.distance).or_insert(0) += 1;
        }
    }
    let mut dist_sorted: Vec<(u32, u32)> = dist_freq.into_iter().collect();
    dist_sorted.sort_by(|a, b| b.1.cmp(&a.1));
    print!("  top distances: ");
    for (d, count) in dist_sorted.iter().take(8) {
        print!("d={d}×{count}, ");
    }
    println!();
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/csv-synthetic.csv".to_string());
    let full = std::fs::read(&path).expect("read csv");
    let n = 65536.min(full.len());
    let input = &full[..n];
    println!("Input: {n} bytes\n");

    for &q in &[1, 5, 11] {
        let is_text = true;
        // Try both default config and a maxed-out config to see if depth helps.
        for &(label_depth, max_chain, nice_match, hash_log) in &[
            ("default", 8u32, 32u32, 17u32),
            ("deep", 1024u32, 1024u32, 18u32),
        ] {
            let _ = is_text;
            let _ = q;
            let cfg = HashChainConfig {
                dict_size: 16 * 1024 * 1024,
                min_match: 4,
                max_chain_length: max_chain,
                nice_match,
                hash_log,
                max_match_length: 271,
                hash_bytes: 4,
            };
            let mut mf = HashChainMatchFinder::new(input, cfg);
            let cmds = _parse_input_with_offset_diag(input, &mut mf, 0, q, false);
            cmd_stats(&cmds, n, &format!("Q{q} {label_depth}"));
        }
        println!();
    }
}
