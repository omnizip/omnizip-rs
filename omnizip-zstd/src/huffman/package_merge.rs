//! Length-limited Huffman code construction via the package-merge
//! algorithm (Larmore & Hirschberg, 1990).
//!
//! Produces optimal code lengths subject to a maximum length constraint.
//! Unlike ad-hoc redistribution heuristics, package-merge guarantees
//! the minimum-redundancy solution: no other length-limited code can
//! have a smaller `Σ freq[i] × length[i]`.
//!
//! ## Algorithm
//!
//! For an alphabet of N symbols with frequencies, and a maximum code
//! length L:
//!
//! 1. Sort symbols by frequency (ascending).
//! 2. Build L "coin lists". Each list is sorted by weight. List 0
//!    contains the original symbols. List k+1 is built by "merging"
//!    pairs from list k (packaging) and adding the original symbols
//!    (merging), keeping the 2N-2 lightest.
//! 3. Count how often each symbol appears in the final list. Each
//!    occurrence corresponds to one bit of code length.
//!
//! Runs in O(N·L) time and O(N·L) space.

#![forbid(unsafe_code)]

/// Compute optimal length-limited Huffman code lengths.
///
/// `freqs` and `lengths` must have the same length N. On return,
/// `lengths[i]` is in `0..=max_len` for each i where `freqs[i] > 0`,
/// and 0 otherwise. The sum of `2^(max_len - lengths[i])` over present
/// symbols equals `2^max_len` (Kraft inequality holds exactly).
///
/// # Panics
///
/// Panics if `freqs.len() != lengths.len()` or `max_len == 0`.
pub fn package_merge(freqs: &[u32], max_len: u8, lengths: &mut [u8]) {
    assert_eq!(freqs.len(), lengths.len(), "freqs/lengths length mismatch");
    assert!(max_len > 0, "max_len must be > 0");

    let n = freqs.len();
    for l in lengths.iter_mut() {
        *l = 0;
    }
    if n == 0 {
        return;
    }

    // Index present symbols and sort by frequency ascending.
    let mut present: Vec<(u32, usize)> = freqs
        .iter()
        .enumerate()
        .filter(|(_, &f)| f > 0)
        .map(|(i, &f)| (f, i))
        .collect();
    let m = present.len();
    if m == 0 {
        return;
    }
    if m == 1 {
        // A single symbol: assign length 1 (or max_len if 1 > max_len).
        lengths[present[0].1] = 1;
        return;
    }
    present.sort_unstable_by_key(|&(f, _)| f);

    // package-merge with capacity bound = 2 * (m - 1).
    let bound = 2 * (m - 1);

    // `list` holds coins for the current level. Each coin is (weight,
    // set_of_symbol_indices). We represent the set as a Vec<usize>.
    // Starting list: the original symbols, sorted by frequency.
    let list: Vec<(u64, Vec<usize>)> = present
        .iter()
        .map(|&(f, i)| (u64::from(f), vec![i]))
        .collect();

    // Previous level's packaged coins (empty for level 0).
    let mut prev_packaged: Vec<(u64, Vec<usize>)> = Vec::new();

    for _level in 1..=max_len {
        // Package: pair up prev_packaged and sort. Each package's weight
        // is the sum of its pair. The symbol set is the union.
        let mut packaged: Vec<(u64, Vec<usize>)> = Vec::with_capacity(prev_packaged.len() / 2);
        for chunk in prev_packaged.chunks_exact(2) {
            let mut combined = chunk[0].1.clone();
            combined.extend_from_slice(&chunk[1].1);
            packaged.push((chunk[0].0 + chunk[1].0, combined));
        }

        // Merge original symbols (sorted) with packaged (sorted).
        // Since `present` is sorted and `packaged` is in pair order
        // (which preserves sorted order), we can merge in linear time.
        // For simplicity, merge + sort.
        let mut merged: Vec<(u64, Vec<usize>)> = Vec::with_capacity(list.len() + packaged.len());
        for (f, i) in present.iter() {
            merged.push((u64::from(*f), vec![*i]));
        }
        for pkg in packaged {
            merged.push(pkg);
        }
        merged.sort_unstable_by_key(|(w, _)| *w);

        // Keep only the `bound` lightest.
        merged.truncate(bound);
        prev_packaged = merged;
    }

    // Count occurrences of each symbol across all coins.
    for (_, syms) in &prev_packaged {
        for &s in syms {
            lengths[s] += 1;
        }
    }

    // Sanity: lengths should be in 0..=max_len.
    debug_assert!(
        lengths.iter().all(|&l| l <= max_len),
        "package-merge produced length > {max_len}: {lengths:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        let mut lengths = vec![];
        package_merge(&[], 11, &mut lengths);
        assert!(lengths.is_empty());
    }

    #[test]
    fn single_symbol() {
        let freqs = [10u32];
        let mut lengths = [0u8];
        package_merge(&freqs, 11, &mut lengths);
        assert_eq!(lengths, [1]);
    }

    #[test]
    fn two_symbols_equal_freq() {
        let freqs = [5u32, 5];
        let mut lengths = [0u8, 0];
        package_merge(&freqs, 11, &mut lengths);
        assert_eq!(lengths, [1, 1]);
    }

    #[test]
    fn three_symbols_kraft_invariant() {
        let freqs = [10u32, 5, 2];
        let mut lengths = [0u8, 0, 0];
        package_merge(&freqs, 11, &mut lengths);
        // Verify Kraft: sum(2^(max_len - l)) == 2^max_len.
        let max = lengths.iter().copied().max().unwrap_or(0);
        let kraft: u64 = lengths
            .iter()
            .filter(|&&l| l > 0)
            .map(|&l| 1u64 << (max - l))
            .sum();
        assert_eq!(kraft, 1u64 << max, "Kraft violated: lengths={lengths:?}");
    }

    #[test]
    fn skewed_distribution_respects_length_limit() {
        // One very-frequent symbol, many rare symbols.
        let mut freqs = vec![1_000_000u32];
        freqs.extend(vec![1u32; 200]);
        let mut lengths = vec![0u8; freqs.len()];
        package_merge(&freqs, 11, &mut lengths);
        // Most-frequent symbol gets shortest code.
        assert!(lengths[0] <= lengths[1]);
        // All lengths ≤ 11.
        assert!(lengths.iter().all(|&l| l <= 11));
        // Kraft holds.
        let max = lengths.iter().copied().max().unwrap_or(0);
        let kraft: u64 = lengths
            .iter()
            .filter(|&&l| l > 0)
            .map(|&l| 1u64 << (max - l))
            .sum();
        assert_eq!(kraft, 1u64 << max);
    }

    #[test]
    fn uniform_distribution_many_symbols() {
        // 256 symbols all equal → all lengths should be the same (= log2(256) = 8).
        let freqs = vec![10u32; 256];
        let mut lengths = vec![0u8; 256];
        package_merge(&freqs, 11, &mut lengths);
        assert!(lengths.iter().all(|&l| l == 8), "expected all 8, got max={:?}", lengths.iter().max());
    }

    #[test]
    fn optimal_cost_beats_or_matches_huffman_with_limit() {
        // For a small alphabet, the package-merge cost (Σ f·l) must be
        // ≤ the cost of any valid length-limited code.
        let freqs = [100u32, 50, 20, 10, 5, 5, 5, 5];
        let mut lengths = [0u8; 8];
        package_merge(&freqs, 3, &mut lengths);
        let pm_cost: u64 = freqs.iter().zip(&lengths).map(|(f, l)| u64::from(*f) * u64::from(*l)).sum();

        // Try all length assignments with max=3 and verify package_merge
        // is optimal (brute-force).
        let n = freqs.len();
        let mut best = u64::MAX;
        for bits in 0u32..(1u32 << (3 * n)) {
            let mut ls = [0u8; 8];
            let mut kraft = 0u64;
            let mut valid = true;
            for i in 0..n {
                let l = ((bits >> (i * 3)) & 0x7) as u8;
                if l == 0 || l > 3 {
                    valid = false;
                    break;
                }
                ls[i] = l;
                kraft += 1u64 << (3 - l);
            }
            if valid && kraft == 1u64 << 3 {
                let cost: u64 = freqs.iter().zip(&ls).map(|(f, l)| u64::from(*f) * u64::from(*l)).sum();
                if cost < best {
                    best = cost;
                }
            }
        }
        assert_eq!(pm_cost, best, "package-merge not optimal");
    }
}
