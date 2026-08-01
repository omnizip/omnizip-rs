//! Length decoder — SDK length-coding scheme.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/lzma/length_coder.rb`
//! (172 LOC, MIT, Ribose Inc.). Decode half only; the encode half
//! lands with the encoder port (Phase B).
//!
//! ## Coding scheme
//!
//! - Lengths 0..=7 (raw, before adding `MATCH_LEN_MIN = 2`):
//!   `choice=0`, then 3 bits from the position-state-keyed low tree.
//! - Lengths 8..=15: `choice=1`, `choice2=0`, then 3 bits from the mid tree.
//! - Lengths 16..=271: `choice=1`, `choice2=1`, then 8 bits from the
//!   shared high tree.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::bit_model::BitModel;
use crate::coder::decoder::decode_tree;
use crate::constants::{
    LEN_LOW_SYMBOLS, LEN_MID_SYMBOLS, NUM_LEN_HIGH_BITS, NUM_LEN_LOW_BITS, NUM_LEN_MID_BITS,
};
use crate::LzmaError;
use crate::RangeDecoder;

/// SDK-style length decoder. The model array is partitioned:
/// `choice`, `choice2`, low trees (one per position state), mid trees
/// (one per position state), high tree (shared).
#[derive(Debug)]
pub struct LengthDecoder {
    num_pos_states: usize,
    choice: BitModel,
    choice2: BitModel,
    /// `num_pos_states` low trees, each of size `2^(NUM_LEN_LOW_BITS + 1)`.
    low: Vec<BitModel>,
    /// `num_pos_states` mid trees, each of size `2^(NUM_LEN_MID_BITS + 1)`.
    mid: Vec<BitModel>,
    /// Single high tree of size `2^(NUM_LEN_HIGH_BITS + 1)`.
    high: Vec<BitModel>,
}

impl LengthDecoder {
    /// Allocate a fresh decoder for `num_pos_states` position states
    /// (typically `1 << pb`).
    ///
    /// # Panics
    ///
    /// Panics if `num_pos_states == 0`.
    #[must_use]
    pub fn new(num_pos_states: usize) -> Self {
        assert!(num_pos_states > 0, "length coder needs ≥1 position state");
        let low_tree_size = 1usize << (NUM_LEN_LOW_BITS + 1);
        let mid_tree_size = 1usize << (NUM_LEN_MID_BITS + 1);
        let high_tree_size = 1usize << (NUM_LEN_HIGH_BITS + 1);
        Self {
            num_pos_states,
            choice: BitModel::new(),
            choice2: BitModel::new(),
            low: vec![BitModel::new(); num_pos_states * low_tree_size],
            mid: vec![BitModel::new(); num_pos_states * mid_tree_size],
            high: vec![BitModel::new(); high_tree_size],
        }
    }

    /// Number of position states this decoder was constructed for.
    #[must_use]
    pub const fn num_pos_states(&self) -> usize {
        self.num_pos_states
    }

    /// Reset every model to the initial probability. Matches XZ Utils'
    /// `reset_models` semantics for the state-reset control packet.
    pub fn reset_models(&mut self) {
        self.choice.reset();
        self.choice2.reset();
        for m in &mut self.low {
            m.reset();
        }
        for m in &mut self.mid {
            m.reset();
        }
        for m in &mut self.high {
            m.reset();
        }
    }

    /// Decode a length value. The result is *raw* (before adding
    /// `MATCH_LEN_MIN`); callers add `MATCH_LEN_MIN` to get the actual
    /// match length.
    ///
    /// # Errors
    ///
    /// Forwards any [`LzmaError`] from the range decoder.
    ///
    /// # Panics
    ///
    /// Panics if `pos_state >= num_pos_states`.
    pub fn decode(
        &mut self,
        range_decoder: &mut RangeDecoder<'_>,
        pos_state: usize,
    ) -> Result<u32, LzmaError> {
        assert!(
            pos_state < self.num_pos_states,
            "pos_state {pos_state} out of range ({})",
            self.num_pos_states,
        );

        let low_tree_size = 1usize << (NUM_LEN_LOW_BITS + 1);
        let mid_tree_size = 1usize << (NUM_LEN_MID_BITS + 1);

        if range_decoder.decode_bit(&mut self.choice)? == 0 {
            let base = pos_state * low_tree_size;
            decode_tree(range_decoder, &mut self.low[base..], NUM_LEN_LOW_BITS)
        } else if range_decoder.decode_bit(&mut self.choice2)? == 0 {
            let base = pos_state * mid_tree_size;
            Ok(LEN_LOW_SYMBOLS
                + decode_tree(range_decoder, &mut self.mid[base..], NUM_LEN_MID_BITS)?)
        } else {
            Ok(LEN_LOW_SYMBOLS
                + LEN_MID_SYMBOLS
                + decode_tree(range_decoder, &mut self.high, NUM_LEN_HIGH_BITS)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_correct_table_sizes() {
        let d = LengthDecoder::new(16);
        // 16 pos states × 16 (low tree) = 256 low models
        assert_eq!(d.low.len(), 16 * (1 << (NUM_LEN_LOW_BITS + 1)));
        // 16 × 16 (mid tree) = 256 mid models
        assert_eq!(d.mid.len(), 16 * (1 << (NUM_LEN_MID_BITS + 1)));
        // 1 × 512 (high tree)
        assert_eq!(d.high.len(), 1usize << (NUM_LEN_HIGH_BITS + 1));
    }

    #[test]
    fn reset_restores_all_models_to_init() {
        let mut d = LengthDecoder::new(4);
        // Touch a few models.
        d.choice.update(0);
        d.choice2.update(1);
        for m in &mut d.low {
            m.update(0);
        }
        d.reset_models();
        assert_eq!(d.choice.probability(), crate::constants::INIT_PROBS);
        for m in &d.low {
            assert_eq!(m.probability(), crate::constants::INIT_PROBS);
        }
    }

    #[test]
    #[should_panic(expected = "length coder needs")]
    fn rejects_zero_pos_states() {
        let _ = LengthDecoder::new(0);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn rejects_pos_state_overflow() {
        let mut d = LengthDecoder::new(2);
        // Provide enough bytes to satisfy any decode_bit calls without
        // panicking on truncation; the panic we expect comes from the
        // pos_state assertion before decode bits are read.
        let mut input = vec![0u8; 32];
        let mut rd = RangeDecoder::new(&input).unwrap();
        let _ = d.decode(&mut rd, 5);
        // Silence unused-mut warning on `input`.
        input.clear();
    }
}
