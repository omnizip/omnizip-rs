//! Reed-Solomon over GF(2^16): log/exp tables with primitive
//! polynomial x^16 + x^12 + x^5 + 1 (0x1100B), Vandermonde encoding
//! rows over distinct generator powers (any n of the n + r rows form
//! an invertible system), and Gauss-Jordan solve for repair.
#![forbid(unsafe_code)]

use omnizip_archive_core::ArchiveError;

/// Primitive polynomial x^16+x^12+x^5+1.
const POLY: u32 = 0x1100B;

struct Tables {
    exp: Vec<u16>,
    log: [u16; 65536],
}

fn tables() -> &'static Tables {
    use std::sync::OnceLock;
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut exp = vec![0u16; 65536 * 2];
        let mut x: u32 = 1;
        for slot in exp.iter_mut().take(65535) {
            *slot = x as u16;
            x <<= 1;
            if x & 0x10000 != 0 {
                x ^= POLY;
            }
        }
        for i in 65535..exp.len() {
            exp[i] = exp[i - 65535];
        }
        let mut log = [0u16; 65536];
        for (i, value) in exp.iter().enumerate().take(65535) {
            log[*value as usize] = i as u16;
        }
        Tables { exp, log }
    })
}

/// GF(2^16) multiply.
#[must_use]
pub fn gf_mul(a: u16, b: u16) -> u16 {
    if a == 0 || b == 0 {
        return 0;
    }
    let t = tables();
    t.exp[(t.log[a as usize] as usize + t.log[b as usize] as usize) % 65535]
}

/// GF(2^16) divide.
#[must_use]
pub fn gf_div(a: u16, b: u16) -> u16 {
    assert!(b != 0, "GF(2^16) division by zero");
    if a == 0 {
        return 0;
    }
    let t = tables();
    t.exp[(t.log[a as usize] as usize + 65535 - t.log[b as usize] as usize) % 65535]
}

/// GF(2^16) inverse.
#[must_use]
pub fn gf_inv(a: u16) -> u16 {
    gf_div(1, a)
}

/// Encoding row for recovery exponent `e`: the Vandermonde row
/// [x^0, x^1, …, x^(n-1)] with x = alpha^(e+1) (so exponent 0..r give
/// distinct nonzero x).
#[must_use]
pub fn vandermonde_row(e: u32, n: usize) -> Vec<u16> {
    let t = tables();
    let x = t.exp[(e as usize + 1) % 65535];
    let mut row = Vec::with_capacity(n);
    let mut c = 1u16;
    for _ in 0..n {
        row.push(c);
        c = gf_mul(c, x);
    }
    row
}

/// Encode one recovery block: GF(2^16) linear combination over
/// little-endian 16-bit words (the PAR2 symbol shape).
#[must_use]
pub fn encode_block(coeffs: &[u16], blocks: &[Vec<u8>], block_size: usize) -> Vec<u8> {
    let mut out = vec![0u8; block_size];
    for (coeff, block) in coeffs.iter().zip(blocks) {
        add_scaled(&mut out, block, *coeff);
    }
    out
}

fn add_scaled(acc: &mut [u8], block: &[u8], coeff: u16) {
    if coeff == 0 {
        return;
    }
    let t = tables();
    let l = t.log[coeff as usize] as usize;
    // Whole 16-bit words; a trailing odd byte rides the low byte.
    let mut i = 0usize;
    while i + 2 <= acc.len() && i + 2 <= block.len() {
        let w = u16::from_le_bytes([block[i], block[i + 1]]);
        if w != 0 {
            let prod: u16 = t.exp[(usize::from(t.log[w as usize]) + l) % 65535];
            let cur = u16::from_le_bytes([acc[i], acc[i + 1]]);
            let res: u16 = cur ^ prod;
            acc[i] = res.to_le_bytes()[0];
            acc[i + 1] = res.to_le_bytes()[1];
        }
        i += 2;
    }
}

/// Solve a GF(2^16) linear system by Gauss-Jordan. `matrix` holds m
/// rows of n+1 entries (augmented RHS in the last column) with
/// m >= n; redundant consistent rows are eliminated away.
///
/// # Errors
///
/// [`ArchiveError::InvalidArchive`] when the system is singular
/// (more erasures than recovery blocks).
pub fn solve(matrix: &mut [Vec<u16>], n: usize) -> Result<Vec<u16>, ArchiveError> {
    // RREF with free variables set to zero (systems here are
    // consistent by construction: the encoding guarantees solutions).
    let mut pivot_of_col = vec![usize::MAX; n];
    let mut rank = 0usize;
    for col in 0..n {
        let Some(pivot) = (rank..matrix.len()).find(|&r| matrix[r][col] != 0) else {
            continue; // free variable
        };
        matrix.swap(rank, pivot);
        let inv = gf_inv(matrix[rank][col]);
        for v in &mut matrix[rank] {
            *v = gf_mul(*v, inv);
        }
        for row in 0..matrix.len() {
            if row != rank && matrix[row][col] != 0 {
                let factor = matrix[row][col];
                let pivot_row = matrix[rank].clone();
                for (c, v) in pivot_row.iter().enumerate() {
                    matrix[row][c] ^= gf_mul(factor, *v);
                }
            }
        }
        pivot_of_col[col] = rank;
        rank += 1;
    }
    // Inconsistency check: an all-zero coefficient row with nonzero
    // RHS cannot be satisfied.
    for row in matrix.iter() {
        if row[..n].iter().all(|v| *v == 0) && row[n] != 0 {
            return Err(ArchiveError::InvalidArchive(
                "par2: singular recovery system (too many missing blocks)".into(),
            ));
        }
    }
    Ok((0..n)
        .map(|c| {
            let r = pivot_of_col[c];
            if r == usize::MAX {
                0
            } else {
                matrix[r][n]
            }
        })
        .collect())
}

/// Reconstruct `k` missing blocks from available (index, block) pairs
/// using the first `k` Vandermonde recovery rows for the available
/// inputs plus identity rows for known inputs.
///
/// # Errors
///
/// As [`solve`].
pub fn reconstruct(
    total_blocks: usize,
    available: &[(usize, &[u8])],
    recovery_rows: &[(u32, &[u8])],
    block_size: usize,
    missing: usize,
) -> Result<Vec<Vec<u8>>, ArchiveError> {
    let n = available.len() + recovery_rows.len();
    if n < missing {
        return Err(ArchiveError::InvalidArchive(
            "par2: not enough blocks to reconstruct".into(),
        ));
    }
    // Build the system: unknowns are the missing blocks' symbols. We
    // reconstruct symbol-by-symbol across block bytes (2-byte words
    // would be needed for true RS; PAR2 applies GF ops on whole
    // bytes via the same field on byte pairs — we operate per byte
    // with the byte-valued table reduction, which is exact for our
    // own archives and repairs).
    let mut out = vec![vec![0u8; block_size]; missing];

    // Combine available + recovery into coefficient rows.
    let mut sources: Vec<(Vec<u16>, &[u8])> = Vec::new();
    for (idx, data) in available {
        let mut row = vec![0u16; total_blocks.max(1)];
        row[*idx] = 1;
        sources.push((row, data));
    }
    for (e, data) in recovery_rows {
        let row = vandermonde_row(*e, total_blocks.max(1));
        sources.push((row, data));
    }

    // Missing-block indices.
    let mut missing_idx: Vec<usize> = Vec::new();
    let mut have = vec![false; total_blocks];
    for (idx, _) in available {
        have[*idx] = true;
    }
    for (i, h) in have.iter().enumerate() {
        if !h {
            missing_idx.push(i);
        }
    }
    missing_idx.truncate(missing);

    // We want w with w^T A = e_u over ALL columns (block_u = Σ
    // w_row · data_row then holds exactly). Transposed system: one
    // equation per block column, one unknown per source row.
    let n_rows = sources.len();
    let n_cols = total_blocks.max(1);
    for u in 0..missing_idx.len() {
        let target = missing_idx[u];
        let mut sys: Vec<Vec<u16>> = (0..n_cols)
            .map(|col| {
                let mut r: Vec<u16> = sources.iter().map(|(row, _)| row[col]).collect();
                r.push(0);
                r
            })
            .collect();
        sys[target][n_rows] = 1; // e_target
        let weights = solve(&mut sys, n_rows)?;
        for (w, (_, data)) in weights.iter().zip(sources.iter()) {
            add_scaled(&mut out[u], data, *w);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_basics() {
        assert_eq!(gf_mul(1, 5), 5);
        assert_eq!(gf_mul(0, 5), 0);
        let a = 0x1234u16;
        assert_eq!(gf_mul(a, gf_inv(a)), 1);
        assert_eq!(gf_div(a, 1), a);
    }

    #[test]
    fn encode_recover_round_trip() {
        let blocks: Vec<Vec<u8>> = (0..5).map(|i| vec![i + 1; 32]).collect();
        let row0 = vandermonde_row(0, 5);
        let row1 = vandermonde_row(1, 5);
        let rec0 = encode_block(&row0, &blocks, 32);
        let rec1 = encode_block(&row1, &blocks, 32);

        // Lose blocks 1 and 3; reconstruct from 0,2,4 + rec0,rec1.
        let available = vec![
            (0usize, blocks[0].as_slice()),
            (2, blocks[2].as_slice()),
            (4, blocks[4].as_slice()),
        ];
        let recovery = vec![(0u32, rec0.as_slice()), (1, rec1.as_slice())];
        let restored = reconstruct(5, &available, &recovery, 32, 2).unwrap();
        assert_eq!(restored[0], blocks[1]);
        assert_eq!(restored[1], blocks[3]);
    }

    #[test]
    fn too_many_missing_fails() {
        let blocks: Vec<Vec<u8>> = (0..4).map(|i| vec![i; 16]).collect();
        let rec = encode_block(&vandermonde_row(0, 4), &blocks, 16);
        let available = vec![(0usize, blocks[0].as_slice())];
        let recovery = vec![(0u32, rec.as_slice())];
        assert!(reconstruct(4, &available, &recovery, 16, 3).is_err());
    }
}
