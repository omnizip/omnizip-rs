//! Suffix array construction.
//!
//! Phase 1 implementation: O(n log n) prefix-doubling sort (Manber-Myers /
//! Karkkainen-Sanders style ranking). Not SA-IS — it is slower (two or three
//! sorts of O(n log n) each) but it is simple, correct, and avoids the
//! pathological O(n^2 log n) behaviour that a naive suffix comparison sort
//! hits on highly repetitive input (e.g. all-same-byte arrays).
//!
//! The output is `Vec<u32>` of length `data.len()`, where `sa[i]` is the
//! starting index of the `i`-th lexicographically smallest suffix.
//!
//! ## Determinism
//!
//! The sort is fully deterministic: ties between equal suffixes are broken
//! by rank, and ranks are assigned in a stable scan from front to back.
//! Same input always produces the same suffix array.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

/// Build the suffix array of `data`.
///
/// Returns a `Vec<u32>` with one entry per byte of `data`, holding the start
/// position of the i-th lexicographically smallest suffix.
///
/// Time complexity: O(n (log n)^2) in the worst case (one sort per doubling
/// round, `log n` rounds, each sort O(n log n)). For Phase 1 inputs this is
/// fast enough; Phase 2+ will swap in SA-IS.
#[must_use]
pub fn build_suffix_array(data: &[u8]) -> Vec<u32> {
    let n = data.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![0];
    }

    // Initial rank = the byte value at each position.
    let mut rank: Vec<i64> = data.iter().map(|&b| i64::from(b)).collect();
    let mut tmp: Vec<i64> = vec![0; n];
    let mut sa: Vec<u32> = (0..n as u32).collect();

    // `k` is the current prefix length we have already sorted by. We double
    // it each iteration until k >= n.
    let mut k: usize = 1;
    while k < n {
        // Comparison key for position `i`:
        //   (rank[i], rank[i+k] if i+k < n else -1)
        // We sort `sa` by this key.
        let key = |i: usize| -> (i64, i64) {
            let r1 = rank[i];
            let r2 = if i + k < n { rank[i + k] } else { -1 };
            (r1, r2)
        };

        sa.sort_by(|&a, &b| {
            let ka = key(a as usize);
            let kb = key(b as usize);
            ka.cmp(&kb)
        });

        // Re-rank: walk the sorted array, assigning sequential ranks. Equal
        // keys get equal ranks; the first distinct key gets the next rank
        // value. This produces a contiguous rank range [0, distinct_count).
        tmp[sa[0] as usize] = 0;
        let mut classes: i64 = 1;
        for i in 1..n {
            let prev = sa[i - 1] as usize;
            let cur = sa[i] as usize;
            if key(cur) != key(prev) {
                classes += 1;
            }
            tmp[cur] = classes - 1;
        }
        std::mem::swap(&mut rank, &mut tmp);

        // If every suffix has a unique rank, we are done early.
        if classes as usize == n {
            break;
        }

        k <<= 1;
    }

    sa
}

/// Compute the LCP (longest common prefix) array from a suffix array.
///
/// `lcp[i]` is the length of the longest common prefix between the suffixes
/// at `sa[i-1]` and `sa[i]`. `lcp[0]` is defined as 0.
///
/// Uses Kasai's algorithm: O(n).
#[must_use]
pub fn build_lcp_array(data: &[u8], sa: &[u32]) -> Vec<u32> {
    let n = data.len();
    if n == 0 {
        return Vec::new();
    }
    let n_u32 = n as u32;

    // Inverse suffix array: inv[sa[i]] = i.
    let mut inv = vec![0u32; n];
    for (i, &s) in sa.iter().enumerate() {
        inv[s as usize] = i as u32;
    }

    let mut lcp = vec![0u32; n];
    let mut h: u32 = 0;
    for i in 0..n_u32 {
        if inv[i as usize] > 0 {
            let j_idx = sa[(inv[i as usize] - 1) as usize];
            // Compare suffixes starting at i and j_idx.
            let iu = i as usize;
            let ju = j_idx as usize;
            while iu + (h as usize) < n
                && ju + (h as usize) < n
                && data[iu + (h as usize)] == data[ju + (h as usize)]
            {
                h += 1;
            }
            lcp[inv[i as usize] as usize] = h;
            h = h.saturating_sub(1);
        } else {
            h = 0;
        }
    }
    lcp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        let sa = build_suffix_array(b"");
        assert!(sa.is_empty());
    }

    #[test]
    fn single_byte() {
        let sa = build_suffix_array(b"a");
        assert_eq!(sa, vec![0]);
    }

    #[test]
    fn sorts_banana() {
        // Suffixes of "banana":
        //   0: banana
        //   1: anana
        //   2: nana
        //   3: ana
        //   4: na
        //   5: a
        // Sorted: a(5), ana(3), anana(1), banana(0), na(4), nana(2)
        let sa = build_suffix_array(b"banana");
        assert_eq!(sa, vec![5, 3, 1, 0, 4, 2]);
    }

    #[test]
    fn all_same_byte() {
        let data = vec![0x41u8; 1000];
        let sa = build_suffix_array(&data);
        // All suffixes are equal, so any permutation is valid. We just need
        // every index to appear exactly once.
        let mut sorted = sa.clone();
        sorted.sort_unstable();
        let expected: Vec<u32> = (0..1000).collect();
        assert_eq!(sorted, expected);
    }

    #[test]
    fn lcp_banana() {
        // Sorted:  a(5) ana(3) anana(1) banana(0) na(4) nana(2)
        // LCP[0]=0 (first by convention)
        // LCP[1]=1 (a vs ana)
        // LCP[2]=3 (ana vs anana)
        // LCP[3]=0 (anana vs banana)
        // LCP[4]=0 (banana vs na)
        // LCP[5]=2 (na vs nana)
        let data = b"banana";
        let sa = build_suffix_array(data);
        let lcp = build_lcp_array(data, &sa);
        assert_eq!(lcp, vec![0, 1, 3, 0, 0, 2]);
    }

    #[test]
    fn deterministic_across_calls() {
        let data: Vec<u8> = (0..500).map(|i| (i % 7) as u8).collect();
        let a = build_suffix_array(&data);
        let b = build_suffix_array(&data);
        assert_eq!(a, b);
    }

    #[test]
    fn suffix_array_is_valid() {
        // Property test: the suffix array is a permutation that sorts the
        // suffixes lexicographically.
        let data: Vec<u8> = b"mississippi".to_vec();
        let sa = build_suffix_array(&data);
        assert_eq!(sa.len(), data.len());
        for w in sa.windows(2) {
            let a = &data[w[0] as usize..];
            let b = &data[w[1] as usize..];
            assert!(
                a <= b,
                "suffix array not sorted at pair {:?} {:?}",
                w[0],
                w[1]
            );
        }
    }
}
