//! Burrows-Wheeler Transform (BWT) — forward and inverse.
//!
//! Port of `omnizip/lib/omnizip/algorithms/bzip2/bwt.rb`.
//!
//! The BWT sorts all rotations of the input block and emits the last column
//! plus a primary index (the row at which the unrotated string sits in the
//! sorted order). Inverse BWT reconstructs the original via the LF mapping.
//!
//! ## Implementation note
//!
//! The Ruby reference sorts rotations with a naive O(n^2) per-comparison
//! routine. That is correct but quadratic; for production use the forward
//! transform here builds the suffix array with the Manber-Myers prefix-
//! doubling algorithm (O(n log^2 n)), which is `unsafe`-free and produces
//! the same lexicographic ordering as the naive sort.

/// Forward BWT.
///
/// Returns `(last_column, primary_index)`. For empty input returns
/// `(empty, 0)`.
#[must_use]
pub fn bwt_encode(data: &[u8]) -> (Vec<u8>, u32) {
    if data.is_empty() {
        return (Vec::new(), 0);
    }

    let sa = build_suffix_array(data);
    let n = data.len();

    // Primary index: rank of rotation starting at offset 0.
    let mut primary_index = 0u32;
    for (rank, &start) in sa.iter().enumerate() {
        if start == 0 {
            primary_index = rank as u32;
            break;
        }
    }

    // Last column: byte preceding each rotation start.
    let last_col: Vec<u8> = sa
        .iter()
        .map(|&start| {
            let prev = if start == 0 { n - 1 } else { start - 1 };
            data[prev]
        })
        .collect();

    (last_col, primary_index)
}

/// Inverse BWT.
///
/// Given the last column and the primary index, reconstruct the original
/// bytes. For empty input returns an empty vec.
///
/// # Errors
///
/// Returns an error message string if `primary_index` is out of range.
pub fn bwt_decode(last_column: &[u8], primary_index: u32) -> Result<Vec<u8>, String> {
    if last_column.is_empty() {
        return Ok(Vec::new());
    }
    let n = last_column.len();
    let primary = primary_index as usize;
    if primary >= n {
        return Err(format!(
            "BWT primary index {primary_index} out of range for length {n}"
        ));
    }

    let lf = build_lf_mapping(last_column);

    // Walk the LF chain. At each step the byte we want is the one whose
    // cumulative range in the sorted order contains the current index — i.e.
    // the sorted (first-column) byte at that position.
    let first_column = sorted_bytes(last_column);

    let mut result = Vec::with_capacity(n);
    let mut idx = primary;
    for _ in 0..n {
        result.push(first_column[idx]);
        idx = lf[idx];
    }
    Ok(result)
}

/// Build the suffix array of `data` treating it as a cyclic string (i.e.
/// sorting rotations, not suffixes). Uses Manber-Myers prefix doubling:
/// O(n log^2 n) comparisons, each O(1).
fn build_suffix_array(data: &[u8]) -> Vec<usize> {
    let n = data.len();
    // rank[i] = lexicographic rank of rotation starting at i, at the current
    // doubling step. Initialised from the first byte of each rotation.
    let mut rank: Vec<u64> = data.iter().map(|&b| u64::from(b)).collect();
    let mut sa: Vec<usize> = (0..n).collect();
    let mut tmp = vec![0u64; n];

    let mut k = 1usize;
    while k < n {
        // Sort by (rank[i], rank[(i+k) % n]).
        let r = &rank;
        sa.sort_by(|&a, &b| {
            let ra = r[a];
            let rb = r[b];
            let rka = r[(a + k) % n];
            let rkb = r[(b + k) % n];
            (ra, rka).cmp(&(rb, rkb))
        });

        // Recompute ranks.
        tmp[sa[0]] = 0;
        for i in 1..n {
            let prev = sa[i - 1];
            let cur = sa[i];
            let prev_key = (rank[prev], rank[(prev + k) % n]);
            let cur_key = (rank[cur], rank[(cur + k) % n]);
            tmp[cur] = tmp[prev] + u64::from(cur_key != prev_key);
        }
        rank.copy_from_slice(&tmp);

        // If every rotation has a unique rank, the sort is complete.
        if rank[sa[n - 1]] == (n - 1) as u64 {
            break;
        }
        k *= 2;
    }

    sa
}

/// Build the LF (Last-to-First) mapping used by inverse BWT.
///
/// For each sorted position `p` in the first column, `lf[p]` is the index in
/// the last column that occupies the same row.
fn build_lf_mapping(last_column: &[u8]) -> Vec<usize> {
    let n = last_column.len();

    let mut counts = [0usize; 256];
    for &b in last_column {
        counts[b as usize] += 1;
    }

    let mut cumulative = [0usize; 256];
    let mut sum = 0;
    for i in 0..256 {
        cumulative[i] = sum;
        sum += counts[i];
    }

    let mut lf = vec![0usize; n];
    let mut occurrence = [0usize; 256];
    for (i, &byte) in last_column.iter().enumerate() {
        let bi = byte as usize;
        let pos_in_first = cumulative[bi] + occurrence[bi];
        occurrence[bi] += 1;
        lf[pos_in_first] = i;
    }
    lf
}

/// Sorted copy of `bytes` — the first column of the BWT rotation matrix.
fn sorted_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut v = bytes.to_vec();
    v.sort_unstable();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_round_trips() {
        let (enc, idx) = bwt_encode(b"");
        assert!(enc.is_empty());
        let dec = bwt_decode(&enc, idx).unwrap();
        assert!(dec.is_empty());
    }

    #[test]
    fn single_byte() {
        let (enc, idx) = bwt_encode(b"A");
        let dec = bwt_decode(&enc, idx).unwrap();
        assert_eq!(dec, b"A");
    }

    #[test]
    fn banana_classic() {
        // "banana" is the canonical BWT example.
        let data = b"banana";
        let (enc, idx) = bwt_encode(data);
        let dec = bwt_decode(&enc, idx).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trip_text() {
        let data = b"The quick brown fox jumps over the lazy dog.";
        let (enc, idx) = bwt_encode(data);
        let dec = bwt_decode(&enc, idx).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trip_repetitive() {
        let data: Vec<u8> = std::iter::repeat(b'a')
            .take(500)
            .chain(std::iter::repeat(b'b').take(500))
            .collect();
        let (enc, idx) = bwt_encode(&data);
        let dec = bwt_decode(&enc, idx).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trip_large_block() {
        // Exercises prefix-doubling on a block big enough to require several
        // doubling passes. Must complete in well under a second.
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        let (enc, idx) = bwt_encode(&data);
        let dec = bwt_decode(&enc, idx).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn rejects_bad_primary_index() {
        let enc = vec![1u8, 2, 3];
        assert!(bwt_decode(&enc, 5).is_err());
    }
}
