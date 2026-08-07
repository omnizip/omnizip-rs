//! bzip2-specific Move-to-Front transform.
//!
//! Unlike the general MTF (which initialises its symbol table with
//! all 256 byte values), bzip2's MTF table is initialised with only
//! the `n_in_use` distinct bytes that appear in the BWT output.
//! This bounds MTF output values to `0..n_in_use-1`, which is what
//! the RLE2/Huffman stages expect.
//!
//! The byte order used to seed the table is the order bytes first
//! appear in the BWT output (or equivalently, ascending byte order —
//! both give the same MTF positions because RLE2 + Huffman are
//! invariant under permutation of the seed order, but ascending
//! byte order matches upstream bzip2).

#![forbid(unsafe_code)]

/// Encode `data` using MTF, seeding the symbol table with only the
/// `seed` distinct bytes (in ascending order).
///
/// Returns the MTF positions in `0..seed.len()`.
///
/// # Panics
///
/// Panics if any byte in `data` is not in `seed`. Caller must ensure
/// `seed` contains exactly the distinct bytes that appear in `data`.
#[must_use]
pub fn mtf_encode_with_seed(data: &[u8], seed: &[u8]) -> Vec<u8> {
    debug_assert!(
        seed.iter()
            .all(|&b| seed.iter().filter(|&&x| x == b).count() == 1),
        "seed must contain unique bytes"
    );

    let mut table: Vec<u8> = seed.to_vec();
    let mut out = Vec::with_capacity(data.len());
    for &byte in data {
        let pos = table
            .iter()
            .position(|&s| s == byte)
            .expect("byte not in seed");
        out.push(pos as u8);
        // Move-to-front: remove and reinsert at index 0.
        table.remove(pos);
        table.insert(0, byte);
    }
    out
}

/// Build the seed for [`mtf_encode_with_seed`]: the distinct bytes in
/// `data`, in ascending order.
#[must_use]
pub fn build_seed(data: &[u8]) -> Vec<u8> {
    let mut seen = [false; 256];
    for &b in data {
        seen[b as usize] = true;
    }
    let mut seed = Vec::new();
    for b in 0..256u16 {
        if seen[b as usize] {
            seed.push(b as u8);
        }
    }
    seed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_data_returns_empty() {
        let seed = build_seed(b"");
        assert!(seed.is_empty());
        let mtf = mtf_encode_with_seed(b"", &seed);
        assert!(mtf.is_empty());
    }

    #[test]
    fn round_trip_three_symbols() {
        let data = b"abcabcabc";
        let seed = build_seed(data);
        assert_eq!(seed, vec![b'a', b'b', b'c']);
        let mtf = mtf_encode_with_seed(data, &seed);
        // 'a' → 0; 'b' → 1, then becomes 0; 'c' → 2, then becomes 0; etc.
        // After three insertions the table is [c,b,a]. Next 'a' is at index 2,
        // moves to front, then 'b' is at index 2 (after [a,c,b]), then 'c'
        // is at index 2.
        assert_eq!(mtf, vec![0, 1, 2, 2, 2, 2, 2, 2, 2]);
    }

    #[test]
    fn positions_bounded_by_seed_len() {
        let data: Vec<u8> = (0..200).map(|i| (i % 5) as u8).collect();
        let seed = build_seed(&data);
        assert_eq!(seed.len(), 5);
        let mtf = mtf_encode_with_seed(&data, &seed);
        assert!(mtf.iter().all(|&v| (v as usize) < seed.len()));
    }

    #[test]
    fn repetitive_input_yields_low_indices() {
        let data = b"aaaaa";
        let seed = build_seed(data);
        assert_eq!(seed, vec![b'a']);
        let mtf = mtf_encode_with_seed(data, &seed);
        // First 'a' is at position 0; subsequent 'a's stay at 0.
        assert_eq!(mtf, vec![0, 0, 0, 0, 0]);
    }
}
