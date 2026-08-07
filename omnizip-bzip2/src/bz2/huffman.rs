//! Canonical Huffman code construction for bzip2.
//!
//! bzip2 uses canonical Huffman codes with code lengths 1..=23 bits.
//! Codes are assigned in order of (length, symbol) — the canonical
//! layout that bzip2's decoder reconstructs from code lengths alone.

#![forbid(unsafe_code)]

/// Maximum allowed Huffman code length in bzip2.
pub const MAX_CODE_LENGTH: u8 = 23;

/// Build canonical Huffman code lengths for `freqs` (indexed by symbol).
///
/// Returns `Vec<u8>` of length `alphabet_size`. All symbols are
/// assigned a non-zero length (matching bzip2's behaviour of replacing
/// zero frequencies with 1 so every symbol has a valid code). Lengths
/// are guaranteed ≤ [`MAX_CODE_LENGTH`].
///
/// Algorithm: standard binary-heap Huffman with iterative rescaling
/// if any length exceeds [`MAX_CODE_LENGTH`]. Deterministic and
/// correct at the cost of slight suboptimality on extreme skews.
#[must_use]
pub fn code_lengths(freqs: &[u32]) -> Vec<u8> {
    let n = freqs.len();
    if n == 0 {
        return Vec::new();
    }

    // bzip2 replaces zero freqs with 1 so every symbol has a code.
    let mut active: Vec<(u32, usize)> = freqs
        .iter()
        .copied()
        .enumerate()
        .map(|(i, f)| (if f == 0 { 1 } else { f }, i))
        .collect();

    if active.len() == 1 {
        let mut out = vec![0u8; n];
        out[active[0].1] = 1;
        return out;
    }

    // Standard Huffman via repeated merging of the two smallest nodes.
    // Each node: (weight, original_symbol or merged_id, depth_indicator).
    // We track the tree implicitly via parent pointers.
    let scale = |freqs: &mut Vec<(u32, usize)>| -> bool {
        let mut scaled = false;
        for (w, _) in freqs.iter_mut() {
            if *w > 1 {
                *w = (*w + 1) / 2;
                scaled = true;
            }
        }
        scaled
    };

    loop {
        let lengths = build_huffman_lengths(&active, n);
        let max_len = lengths.iter().copied().max().unwrap_or(0);
        if max_len <= MAX_CODE_LENGTH {
            return lengths;
        }
        if !scale(&mut active) {
            // Can't reduce further; clamp lengths.
            return lengths
                .into_iter()
                .map(|l| l.min(MAX_CODE_LENGTH))
                .collect();
        }
    }
}

/// Build standard Huffman code lengths via binary-heap merging.
/// Returns lengths indexed by original symbol index (0..alphabet_size).
fn build_huffman_lengths(active: &[(u32, usize)], alphabet_size: usize) -> Vec<u8> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    #[derive(Eq, PartialEq)]
    struct Node {
        weight: u32,
        // Unique id; for leaves this is the original symbol index, for
        // internal nodes it's `alphabet_size + creation_order`.
        id: usize,
    }
    impl Ord for Node {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.weight.cmp(&other.weight).then(self.id.cmp(&other.id))
        }
    }
    impl PartialOrd for Node {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    let mut heap: BinaryHeap<Reverse<Node>> = BinaryHeap::new();
    for &(w, sym) in active {
        heap.push(Reverse(Node { weight: w, id: sym }));
    }

    // parent[id] = parent id, indexed by id.
    let mut parent: Vec<Option<usize>> = vec![None; alphabet_size + active.len()];
    let mut next_id = alphabet_size;

    while heap.len() > 1 {
        let a = heap.pop().unwrap().0;
        let b = heap.pop().unwrap().0;
        let new_id = next_id;
        next_id += 1;
        parent[a.id] = Some(new_id);
        parent[b.id] = Some(new_id);
        heap.push(Reverse(Node {
            weight: a.weight + b.weight,
            id: new_id,
        }));
    }
    let root = heap.pop().unwrap().0.id;

    // Walk from each leaf to the root to compute depth = code length.
    let mut out = vec![0u8; alphabet_size];
    for &(w, sym) in active {
        let _ = w;
        let mut depth = 0u8;
        let mut cur = sym;
        while let Some(p) = parent[cur] {
            depth += 1;
            cur = p;
        }
        if cur == root {
            out[sym] = depth.max(1);
        } else {
            // Unreachable — every leaf should walk to the root.
            out[sym] = 1;
        }
    }
    out
}

/// Compute canonical Huffman codes from code lengths.
///
/// Codes are assigned in order of (length, symbol). Returns
/// `Vec<(u32 code, u8 length)>` indexed by symbol. Unused symbols
/// (length 0) have code 0, length 0.
#[must_use]
pub fn canonical_codes(lengths: &[u8]) -> Vec<(u32, u8)> {
    let n = lengths.len();
    let mut out = vec![(0u32, 0u8); n];

    // Count occurrences of each length.
    let max_len = lengths.iter().copied().max().unwrap_or(0) as usize;
    if max_len == 0 {
        return out;
    }
    let mut bl_count = vec![0u32; max_len + 1];
    for &l in lengths {
        if l > 0 {
            bl_count[l as usize] += 1;
        }
    }

    // Compute first code for each length (canonical Huffman, RFC 1951-style).
    let mut code: u32 = 0;
    let mut next_code = vec![0u32; max_len + 1];
    for bits in 1..=max_len {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }

    // Assign codes to symbols in (symbol-order, since we want deterministic).
    for (sym, &len) in lengths.iter().enumerate() {
        if len > 0 {
            out[sym] = (next_code[len as usize], len);
            next_code[len as usize] += 1;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_returns_empty() {
        assert!(code_lengths(&[]).is_empty());
    }

    #[test]
    fn single_active_symbol_gets_length_one() {
        // bzip2 replaces zero freqs with 1 so every symbol gets a code.
        let f = vec![0u32, 5, 0];
        let l = code_lengths(&f);
        // All three symbols get a length (no zeros); the active one
        // gets the shortest code.
        assert!(l.iter().all(|&x| x >= 1));
        assert_eq!(l[1], 1);
    }

    #[test]
    fn two_symbols_get_length_one_each() {
        let f = vec![5u32, 3];
        let l = code_lengths(&f);
        assert_eq!(l, vec![1, 1]);
    }

    #[test]
    fn skew_respects_length_limit() {
        // Symbol 0 has high freq; others low. With limit 23 bits this
        // should always succeed even for very skewed distributions.
        let mut f = vec![1u32; 200];
        f[0] = 1_000_000;
        let l = code_lengths(&f);
        let max_len = l.iter().copied().max().unwrap_or(0);
        assert!(max_len <= MAX_CODE_LENGTH);
    }

    #[test]
    fn lengths_satisfy_kraft_inequality() {
        // For valid Huffman codes: sum(2^-len) == 1.
        let f = vec![10u32, 5, 8, 1, 2, 6, 3];
        let l = code_lengths(&f);
        let mut sum: f64 = 0.0;
        for &len in &l {
            if len > 0 {
                sum += 1.0 / f64::from(1u32 << len);
            }
        }
        assert!((sum - 1.0).abs() < 1e-9, "Kraft sum = {sum}");
    }

    #[test]
    fn canonical_codes_are_unique_and_have_correct_lengths() {
        let f = vec![8u32, 5, 3, 1, 1, 1];
        let l = code_lengths(&f);
        let codes = canonical_codes(&l);
        let active: Vec<_> = codes.iter().filter(|(_, len)| *len > 0).collect();
        // All codes distinct.
        let mut seen = std::collections::HashSet::new();
        for &(c, _) in active {
            assert!(seen.insert(c), "duplicate code {c}");
        }
        // Lengths match what code_lengths produced.
        for (sym, &(_c, len)) in codes.iter().enumerate() {
            assert_eq!(len, l[sym]);
        }
    }
}
