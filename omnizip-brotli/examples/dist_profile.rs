//! Profile distance distribution to choose optimal NDIRECT.

use omnizip_brotli::from_spec_encoder::_parse_input_with_offset_diag;

fn main() {
    let input = std::fs::read("/tmp/csv-synthetic.csv").expect("read");
    let n = 65536.min(input.len());
    let input = &input[..n];

    use omnizip_codecs::{HashChainConfig, HashChainMatchFinder};
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
    let cmds = _parse_input_with_offset_diag(input, &mut mf, 0, 5, false);

    let mut dist_freq = std::collections::HashMap::new();
    let mut max_dist = 0;
    for c in &cmds {
        if c.copy_len > 0 {
            *dist_freq.entry(c.distance).or_insert(0u32) += 1;
            max_dist = max_dist.max(c.distance);
        }
    }
    println!("Max distance: {max_dist}");
    println!("Distinct distances: {}", dist_freq.len());

    // Count by range
    let ranges = [
        (1, 16),
        (17, 32),
        (33, 64),
        (65, 128),
        (129, 256),
        (257, 512),
        (513, 1024),
        (1025, 4096),
        (4097, 16384),
        (16385, 1 << 24),
    ];
    for (lo, hi) in ranges {
        let count: u32 = dist_freq
            .iter()
            .filter(|(d, _)| **d >= lo && **d <= hi)
            .map(|(_, c)| *c)
            .sum();
        let distinct = dist_freq
            .iter()
            .filter(|(d, _)| **d >= lo && **d <= hi)
            .count();
        println!("  d={lo}-{hi}: {count} occurrences ({distinct} distinct)");
    }

    // What if NDIRECT=64? Distances up to 16+64=80 are direct-coded.
    let direct_count: u32 = dist_freq
        .iter()
        .filter(|(d, _)| **d <= 80)
        .map(|(_, c)| *c)
        .sum();
    let total: u32 = dist_freq.values().sum();
    println!(
        "\nIf NDIRECT=64: {}/{} distances ({:.1}%) would be direct-coded (no extra bits)",
        direct_count,
        total,
        direct_count as f64 * 100.0 / total as f64
    );
}
