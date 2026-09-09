//! Shared Huffman coding primitives.
//!
//! Provides canonical Huffman tree building (with optional length
//! limit via package-merge), encoding, and decoding. Used by ZSTD,
//! Brotli, `BZip2`, and DEFLATE — eliminating ~400 LOC of duplicated
//! Huffman implementations across the workspace.
//!
//! ## Determinism
//!
//! All algorithms are deterministic: same frequencies → same tree.

#![forbid(unsafe_code)]

/// A canonical Huffman code length assignment.
#[derive(Clone, Debug)]
pub struct HuffmanLengths {
    /// Code length per symbol (0 = symbol not used).
    pub lengths: Vec<u8>,
    /// Maximum code length.
    pub max_length: u8,
}

impl HuffmanLengths {
    /// Build canonical Huffman code lengths from symbol frequencies.
    ///
    /// Uses the standard package-merge algorithm when a length limit
    /// is specified, or unrestricted Huffman otherwise.
    ///
    /// Symbols with frequency 0 get length 0 (excluded from the tree).
    #[must_use]
    pub fn build(freqs: &[u32], max_length: u8) -> Self {
        let n = freqs.len();
        if n == 0 {
            return Self {
                lengths: Vec::new(),
                max_length: 0,
            };
        }

        // Collect non-zero symbols.
        let symbols: Vec<(usize, u32)> = freqs
            .iter()
            .enumerate()
            .filter(|(_, &f)| f > 0)
            .map(|(i, &f)| (i, f))
            .collect();

        if symbols.is_empty() {
            return Self {
                lengths: vec![0; n],
                max_length: 0,
            };
        }
        if symbols.len() == 1 {
            let mut lengths = vec![0u8; n];
            lengths[symbols[0].0] = 1;
            return Self {
                lengths,
                max_length: 1,
            };
        }

        // Build tree via package-merge for length-limited codes.
        let lengths = package_merge(&symbols, max_length as usize, n);

        Self {
            lengths,
            max_length,
        }
    }

    /// Generate canonical codes from code lengths.
    /// Returns (code, length) pairs indexed by symbol.
    #[must_use]
    pub fn canonical_codes(&self) -> Vec<(u32, u8)> {
        let n = self.lengths.len();
        let mut codes = vec![(0u32, 0u8); n];

        // Count symbols per length.
        let mut bl_count = vec![0u32; (self.max_length as usize) + 1];
        for &len in &self.lengths {
            if len > 0 {
                bl_count[len as usize] += 1;
            }
        }

        // Compute first code per length.
        let mut next_code = vec![0u32; (self.max_length as usize) + 2];
        let mut code = 0u32;
        bl_count[0] = 0;
        for bits in 1..=self.max_length as usize {
            code = (code + bl_count[bits - 1]) << 1;
            next_code[bits] = code;
        }

        // Assign codes.
        for (sym, &len) in self.lengths.iter().enumerate() {
            if len > 0 {
                codes[sym] = (next_code[len as usize], len);
                next_code[len as usize] += 1;
            }
        }
        codes
    }
}

/// Package-merge algorithm for length-limited Huffman codes.
///
/// Produces optimal code lengths where no code exceeds `max_length` bits.
/// Based on Larmore & Hirschberg (1990).
fn package_merge(symbols: &[(usize, u32)], max_length: usize, n_symbols: usize) -> Vec<u8> {
    let n = symbols.len();
    if n == 0 {
        return vec![0; n_symbols];
    }

    // Sort by frequency.
    let mut sorted: Vec<(usize, u32)> = symbols.to_vec();
    sorted.sort_by_key(|&(_, f)| f);

    // Arena form: a coin is a node index. Leaves carry the symbol;
    // packages carry child indices. The previous form allocated a
    // Vec<usize> of covered symbols per coin, cloned them through the
    // merge loop, and rebuilt the original-coin list every level —
    // the realloc storm behind ~12% of plists-q5 encode samples.
    // Comparisons and tie-breaking are byte-identical to that form,
    // so the emitted lengths are unchanged.
    let cap = 4 * n * max_length.max(1);
    let mut freqs: Vec<u64> = Vec::with_capacity(cap);
    let mut lefts: Vec<u32> = Vec::with_capacity(cap);
    let mut rights: Vec<u32> = Vec::with_capacity(cap);

    fn push_leaf(
        freq: u32,
        sym: usize,
        freqs: &mut Vec<u64>,
        lefts: &mut Vec<u32>,
        rights: &mut Vec<u32>,
    ) -> u32 {
        let idx = freqs.len() as u32;
        freqs.push(u64::from(freq));
        lefts.push(sym as u32);
        rights.push(u32::MAX);
        idx
    }

    // Initial coins: one per symbol.
    let mut prev_list: Vec<u32> = sorted
        .iter()
        .map(|&(sym, freq)| push_leaf(freq, sym, &mut freqs, &mut lefts, &mut rights))
        .collect();

    // Package-merge iterations.
    for _ in 1..max_length {
        // Package: pair up adjacent coins.
        let mut packages: Vec<u32> = Vec::with_capacity(prev_list.len() / 2);
        let mut i = 0;
        while i + 1 < prev_list.len() {
            let l = prev_list[i] as usize;
            let r = prev_list[i + 1] as usize;
            let idx = freqs.len() as u32;
            freqs.push(freqs[l] + freqs[r]);
            lefts.push(prev_list[i]);
            rights.push(prev_list[i + 1]);
            packages.push(idx);
            i += 2;
        }

        // Merge packages with fresh original coins, sorted by
        // frequency (packages win ties, as before).
        let orig: Vec<u32> = sorted
            .iter()
            .map(|&(sym, freq)| push_leaf(freq, sym, &mut freqs, &mut lefts, &mut rights))
            .collect();
        let mut merged: Vec<u32> = Vec::with_capacity(packages.len() + orig.len());
        let mut pi = 0;
        let mut oi = 0;
        while pi < packages.len() && oi < orig.len() {
            if freqs[packages[pi] as usize] <= freqs[orig[oi] as usize] {
                merged.push(packages[pi]);
                pi += 1;
            } else {
                merged.push(orig[oi]);
                oi += 1;
            }
        }
        merged.extend_from_slice(&packages[pi..]);
        merged.extend_from_slice(&orig[oi..]);

        // Keep only first 2*(n-1) coins.
        let limit = 2 * (n - 1);
        if merged.len() > limit {
            merged.truncate(limit);
        }
        prev_list = merged;
    }

    // Count how many times each symbol appears in the final list:
    // occurrences of a coin's leaves across prev_list.
    let mut lengths = vec![0u8; n_symbols];
    let mut stack: Vec<u32> = Vec::new();
    for &coin in &prev_list {
        stack.push(coin);
        while let Some(node) = stack.pop() {
            let node = node as usize;
            if rights[node] == u32::MAX {
                lengths[lefts[node] as usize] += 1;
            } else {
                stack.push(lefts[node]);
                stack.push(rights[node]);
            }
        }
    }

    lengths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_single_symbol() {
        let lengths = HuffmanLengths::build(&[0, 5, 0, 0], 15);
        assert_eq!(lengths.lengths, vec![0, 1, 0, 0]);
    }

    #[test]
    fn build_two_symbols() {
        let lengths = HuffmanLengths::build(&[3, 1], 15);
        assert_eq!(lengths.lengths[0], 1);
        assert_eq!(lengths.lengths[1], 1);
    }

    #[test]
    fn build_respects_max_length() {
        // Fibonacci-like frequencies push code lengths high without limit.
        let freqs: Vec<u32> = (0..30).map(|i| 1u32 << i).collect();
        let lengths = HuffmanLengths::build(&freqs, 11);
        assert!(
            lengths.lengths.iter().all(|&l| l <= 11),
            "all lengths must be ≤ 11, got max {}",
            lengths.lengths.iter().max().copied().unwrap_or(0)
        );
    }

    #[test]
    fn canonical_codes_are_prefix_free() {
        let lengths = HuffmanLengths::build(&[5, 3, 1, 1], 15);
        let codes = lengths.canonical_codes();
        // Verify prefix-free property: no code is a prefix of another.
        for i in 0..codes.len() {
            for j in (i + 1)..codes.len() {
                let (ci, li) = codes[i];
                let (cj, lj) = codes[j];
                if li == 0 || lj == 0 {
                    continue;
                }
                let (shorter, longer, ls) = if li < lj { (ci, cj, li) } else { (cj, ci, lj) };
                let diff = li.max(lj) - ls;
                assert_ne!(
                    shorter,
                    longer >> diff,
                    "code {} is a prefix of code {}",
                    i,
                    j
                );
            }
        }
    }

    #[test]
    fn determinism() {
        let freqs = vec![10, 5, 3, 1, 1];
        let a = HuffmanLengths::build(&freqs, 11);
        let b = HuffmanLengths::build(&freqs, 11);
        assert_eq!(a.lengths, b.lengths);
    }

    #[test]
    fn empty_input() {
        let lengths = HuffmanLengths::build(&[], 15);
        assert!(lengths.lengths.is_empty());
    }

    #[test]
    fn all_zero_frequencies() {
        let lengths = HuffmanLengths::build(&[0, 0, 0], 15);
        assert!(lengths.lengths.iter().all(|&l| l == 0));
    }
}
