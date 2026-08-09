//! bzip2 RLE2 — bijective base-2 (RUNA/RUNB) run encoding of zeros.
//!
//! After MTF, the output is mostly zeros. bzip2 collapses runs of
//! zeros using RUNA/RUNB in bijective base-2:
//!
//! - RUNA = 1
//! - RUNB = 2
//! - A run of length L (L ≥ 1) is encoded by writing the digits of
//!   L in bijective base-2, least-significant digit first.
//! - e.g. L=1 → RUNA; L=2 → RUNB; L=3 → RUNA,RUNA; L=4 → RUNA,RUNB;
//!   L=5 → RUNB,RUNA; L=6 → RUNB,RUNB; L=7 → RUNA,RUNA,RUNA.
//!
//! Non-zero MTF values `v` (`1..=mtf_count`) are emitted as symbol
//! `v + 1` (skipping the two RUN slots). End-of-block is symbol
//! `mtf_count + 2`.
//!
//! Alphabet layout (for nInUse distinct bytes):
//!
//! ```text
//! symbol 0     : RUNA
//! symbol 1     : RUNB
//! symbol 2..=nInUse : MTF value (1..=nInUse-1) → symbol = mtf_value + 1
//! symbol nInUse+1   : EOB
//! ```
//!
//! So total alphabet size is `nInUse + 2`.

#![forbid(unsafe_code)]

/// Symbol value for RUNA.
pub const RUNA: u16 = 0;
/// Symbol value for RUNB.
pub const RUNB: u16 = 1;
/// Offset: MTF value `v ≥ 1` becomes symbol `v + 1`.
pub const MTF_SYMBOL_OFFSET: u16 = 1;

/// Encode the MTF output into the bzip2 Huffman alphabet, appending a
/// final EOB symbol.
///
/// `mtf_values` are the MTF positions in 0..n_in_use-1. `n_in_use`
/// is the number of distinct bytes in the BWT output (size of the
/// active alphabet). Returns the encoded symbol stream with EOB at
/// the end.
#[must_use]
pub fn mtf_to_symbols(mtf_values: &[u8], n_in_use: usize) -> Vec<u16> {
    let eob = n_in_use as u16 + 1; // symbol index of EOB
    let mut out: Vec<u16> = Vec::with_capacity(mtf_values.len() + 1);

    let mut i = 0;
    while i < mtf_values.len() {
        let v = mtf_values[i];
        if v == 0 {
            // Count the run of zeros starting at i.
            let mut run: u32 = 0;
            while i < mtf_values.len() && mtf_values[i] == 0 {
                run = run.saturating_add(1);
                i += 1;
            }
            // Emit `run` in bijective base-2, LSD first.
            // Iteratively subtract 1 then divide by 2, emitting RUNA for
            // quotient-bit 1 and RUNB for quotient-bit 2.
            let mut n = run;
            while n > 0 {
                n -= 1;
                if n & 1 == 0 {
                    out.push(RUNA);
                } else {
                    out.push(RUNB);
                }
                n >>= 1;
            }
        } else {
            // Non-zero MTF value → symbol = v + 1.
            out.push(u16::from(v) + MTF_SYMBOL_OFFSET);
            i += 1;
        }
    }
    out.push(eob);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_run_length(symbols: &[u16]) -> u32 {
        // The bijective base-2 value: sum of (digit value) * 2^position.
        let mut total: u32 = 0;
        let mut weight: u32 = 1;
        for &sym in symbols {
            let digit = if sym == RUNA { 1 } else { 2 };
            total += digit * weight;
            weight *= 2;
        }
        total
    }

    #[test]
    fn single_value_no_zeros_emits_value_plus_one_then_eob() {
        let mtf = vec![3u8];
        let syms = mtf_to_symbols(&mtf, 10);
        assert_eq!(syms, vec![3 + 1, 11]); // 11 = EOB = 10 + 1
    }

    #[test]
    fn single_zero_emits_runa() {
        let mtf = vec![0u8];
        let syms = mtf_to_symbols(&mtf, 10);
        // EOB symbol = 11; run of 1 zero = RUNA.
        assert_eq!(syms, vec![RUNA, 11]);
    }

    #[test]
    fn two_zeros_emit_runb() {
        // Run of 2 zeros = RUNB (bijective base-2: 2 = "2").
        let mtf = vec![0u8, 0];
        let syms = mtf_to_symbols(&mtf, 10);
        assert_eq!(syms, vec![RUNB, 11]);
    }

    #[test]
    fn three_zeros_emit_runa_runa() {
        // Run of 3 zeros = RUNA,RUNA (bijective base-2: 3 = "11" = "RUNA RUNA").
        let mtf = vec![0u8, 0, 0];
        let syms = mtf_to_symbols(&mtf, 10);
        assert_eq!(syms, vec![RUNA, RUNA, 11]);
        assert_eq!(decode_run_length(&[RUNA, RUNA]), 3);
    }

    #[test]
    fn round_trip_run_lengths_via_decode_helper() {
        for run in 1..=20 {
            let mut mtf = vec![0u8; run as usize];
            // Append a non-zero so the run terminates here naturally
            // (otherwise we'd be encoding only the run; but the
            // `mtf_to_symbols` function knows where the run ends via
            // the slice length, so that's fine).
            mtf.push(5);
            let syms = mtf_to_symbols(&mtf, 10);
            // Strip trailing value+1 and EOB to get just the run.
            let run_syms: Vec<u16> = syms
                .iter()
                .copied()
                .take_while(|&s| s == RUNA || s == RUNB)
                .collect();
            assert_eq!(decode_run_length(&run_syms), run);
        }
    }

    #[test]
    fn nonzero_after_run_emits_correct_symbol() {
        let mtf = vec![0u8, 0, 0, 7]; // 3 zeros + 7
        let syms = mtf_to_symbols(&mtf, 10);
        // 3 zeros → RUNA,RUNA; 7 → symbol 8; EOB = 11.
        assert_eq!(syms, vec![RUNA, RUNA, 8, 11]);
    }

    #[test]
    fn empty_mtf_emits_only_eob() {
        let syms = mtf_to_symbols(&[], 5);
        assert_eq!(syms, vec![6]); // EOB = 5 + 1
    }
}
