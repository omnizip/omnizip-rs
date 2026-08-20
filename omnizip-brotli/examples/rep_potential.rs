//! Measure potential ratio improvement from rep-code-aware parsing.

use omnizip_brotli::from_spec_encoder::{Command, _parse_input_with_offset_diag};
use omnizip_codecs::{HashChainConfig, HashChainMatchFinder};

fn measure_rep_potential(cmds: &[Command]) -> (usize, usize) {
    // Simulate: walk commands forward, track rep state (4 distances).
    // Count how many distances could use rep codes vs. how many actually do
    // with greedy rep matching.
    let mut rep = [16u32, 15, 11, 4];
    let mut rep_idx: usize = 0;
    let mut actual_reps = 0usize;
    let mut potential_reps = 0usize;
    let mut total_bits_current = 0usize;
    let mut total_bits_if_all_rep = 0usize;

    for c in cmds {
        if c.copy_len == 0 {
            continue;
        }
        // Check if distance matches any rep
        let mut found_rep = None;
        for (i, r) in rep.iter().enumerate() {
            if *r == c.distance {
                found_rep = Some(i);
                break;
            }
        }
        if found_rep.is_some() {
            actual_reps += 1;
            potential_reps += 1;
            total_bits_current += 3; // approx rep code cost
            total_bits_if_all_rep += 3;
        } else {
            // Estimate explicit distance cost: 5+log2(dist)
            let log_d = (c.distance as f32).ln() / std::f32::consts::LN_2;
            let bits = (5.0 + log_d).min(22.0) as usize;
            total_bits_current += bits;
            total_bits_if_all_rep += 3; // hypothetical
        }

        // Update rep state
        if found_rep.is_some() {
            // promote to rep0
        } else {
            // shift and insert
            rep[3] = rep[2];
            rep[2] = rep[1];
            rep[1] = rep[0];
            rep[0] = c.distance;
            let _ = rep_idx;
        }
    }
    (actual_reps, total_bits_current)
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/csv-synthetic.csv".to_string());
    let full = std::fs::read(&path).expect("read csv");
    let n = 65536.min(full.len());
    let input = &full[..n];

    for &q in &[5] {
        let cfg = HashChainConfig {
            dict_size: 16 * 1024 * 1024,
            min_match: 4,
            max_chain_length: 8,
            nice_match: 32,
            hash_log: 17,
            max_match_length: 271,
            hash_bytes: 4,
        };
        let mut mf = HashChainMatchFinder::new(input, cfg);
        let cmds = _parse_input_with_offset_diag(input, &mut mf, 0, q, false);
        let (reps, bits) = measure_rep_potential(&cmds);
        let total_matches = cmds.iter().filter(|c| c.copy_len > 0).count();
        println!(
            "Q{q}: {total_matches} matches, {reps} rep-code-able ({:.1}%)",
            reps as f64 * 100.0 / total_matches.max(1) as f64
        );
        println!("  estimated distance bits: {bits} = {} bytes", bits / 8);
    }
}
