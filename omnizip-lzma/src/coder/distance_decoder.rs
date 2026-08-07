//! Distance decoder — SDK distance-coding scheme.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/lzma/distance_coder.rb`
//! (326 LOC, MIT, Ribose Inc.). Decode half only.
//!
//! ## Coding scheme
//!
//! - Slot 0..=3: direct (no extra bits) — `distance = slot`.
//! - Slot 4..=13: slot selects a base; `slot >> 1` extra bits decoded
//!   via the reverse-tree position model keyed by `base - slot - 1`.
//! - Slot 14..=63: `(slot >> 1) - 1` footer bits split into
//!   `num_direct_bits = footer - DIST_ALIGN_BITS` high bits (decoded
//!   via `decode_direct_bits_with_base`) and `DIST_ALIGN_BITS` low bits
//!   (decoded via the reverse-tree align model).
//!
//! The returned distance is the value *before* adding 1, matching the
//! Ruby / XZ Utils convention. Callers add 1 to get the actual match
//! distance.

#![forbid(unsafe_code)]

use crate::bit_model::BitModel;
use crate::coder::decoder::{decode_reverse_tree, decode_tree};
use crate::constants::{
    DIST_ALIGN_BITS, END_POS_MODEL_INDEX, NUM_DIST_SLOT_BITS, NUM_FULL_DISTANCES,
    START_POS_MODEL_INDEX,
};
use crate::LzmaError;
use crate::RangeDecoder;

/// Number of length-to-position states used to key the slot encoder.
const DEFAULT_NUM_LEN_TO_POS_STATES: usize = crate::constants::NUM_LEN_TO_POS_STATES as usize;

/// SDK distance decoder. The model array is partitioned:
/// `slot_encoders` (per length state), `pos_encoders` (shared reverse
/// tree for slots 4..=13), `align_encoder` (reverse tree for slots ≥14).
#[derive(Debug)]
pub struct DistanceDecoder {
    /// `num_len_to_pos_states` slot trees, each of size
    /// `2^(NUM_DIST_SLOT_BITS + 1)`.
    slot_encoders: Vec<BitModel>,
    /// Reverse-tree position models for slots 4..=13. Size
    /// `NUM_FULL_DISTANCES - END_POS_MODEL_INDEX`.
    pos_encoders: Vec<BitModel>,
    /// 4-bit aligned reverse-tree for slots ≥14.
    align_encoder: Vec<BitModel>,
    num_len_to_pos_states: usize,
}

impl DistanceDecoder {
    /// Allocate a fresh decoder for the given number of length-to-position
    /// states (typically 4 — see `NUM_LEN_TO_POS_STATES`).
    ///
    /// # Panics
    ///
    /// Panics if `num_len_to_pos_states == 0`.
    #[must_use]
    pub fn new(num_len_to_pos_states: usize) -> Self {
        assert!(
            num_len_to_pos_states > 0,
            "distance coder needs ≥1 length state",
        );
        let slot_tree_size = 1usize << (NUM_DIST_SLOT_BITS + 1);
        let pos_encoder_size = (NUM_FULL_DISTANCES - END_POS_MODEL_INDEX) as usize;
        let align_size = 1usize << (DIST_ALIGN_BITS + 1);
        Self {
            slot_encoders: vec![BitModel::new(); num_len_to_pos_states * slot_tree_size],
            pos_encoders: vec![BitModel::new(); pos_encoder_size],
            align_encoder: vec![BitModel::new(); align_size],
            num_len_to_pos_states,
        }
    }

    /// Construct with the default length-state count
    /// ([`NUM_LEN_TO_POS_STATES`]).
    #[must_use]
    pub fn with_default_states() -> Self {
        Self::new(DEFAULT_NUM_LEN_TO_POS_STATES)
    }

    /// Number of length-to-position states this decoder was built for.
    #[must_use]
    pub const fn num_len_to_pos_states(&self) -> usize {
        self.num_len_to_pos_states
    }

    /// Reset every model to the initial probability.
    pub fn reset_models(&mut self) {
        for m in &mut self.slot_encoders {
            m.reset();
        }
        for m in &mut self.pos_encoders {
            m.reset();
        }
        for m in &mut self.align_encoder {
            m.reset();
        }
    }

    /// Decode a distance. The result is the value *before* adding 1
    /// (i.e. 0 means distance 1 in the dictionary).
    ///
    /// # Errors
    ///
    /// Forwards any [`LzmaError`] from the range decoder.
    ///
    /// # Panics
    ///
    /// Panics if `len_state >= num_len_to_pos_states`.
    pub fn decode(
        &mut self,
        range_decoder: &mut RangeDecoder<'_>,
        len_state: usize,
    ) -> Result<u32, LzmaError> {
        assert!(
            len_state < self.num_len_to_pos_states,
            "len_state {len_state} out of range ({})",
            self.num_len_to_pos_states,
        );

        let slot_tree_size = 1usize << (NUM_DIST_SLOT_BITS + 1);
        let base = len_state * slot_tree_size;
        let slot = decode_tree(
            range_decoder,
            &mut self.slot_encoders[base..],
            NUM_DIST_SLOT_BITS,
        )?;

        if slot < START_POS_MODEL_INDEX {
            return Ok(slot);
        }

        let footer_bits = (slot >> 1) - 1;

        if slot < END_POS_MODEL_INDEX {
            // Slots 4..=13: reverse tree over pos_encoders.
            // NOTE: pos_idx is i64 because for slot 4, base - slot - 1
            // = -1. The tree walk adds m=1 internally, making the
            // effective index 0. See coder/decoder.rs docs.
            let slot_base = (2 | (slot & 1)) << footer_bits;
            let pos_idx = i64::from(slot_base as u32) - i64::from(slot) - 1;
            let extra =
                decode_reverse_tree(range_decoder, &mut self.pos_encoders, pos_idx, footer_bits)?;
            Ok(slot_base + extra)
        } else {
            // Slots ≥14: high direct bits + low aligned bits.
            let num_direct_bits = footer_bits - DIST_ALIGN_BITS;
            let mut result = 2 + (slot & 1);
            result = range_decoder.decode_direct_bits_with_base(num_direct_bits, result)?;
            let low_bits =
                decode_reverse_tree(range_decoder, &mut self.align_encoder, 0, DIST_ALIGN_BITS)?;
            Ok((result << DIST_ALIGN_BITS) + low_bits)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_correct_table_sizes() {
        let d = DistanceDecoder::with_default_states();
        // 4 states × 128 (slot tree) = 512
        assert_eq!(
            d.slot_encoders.len(),
            DEFAULT_NUM_LEN_TO_POS_STATES * (1 << (NUM_DIST_SLOT_BITS + 1))
        );
        // pos_encoders size = NUM_FULL_DISTANCES - END_POS_MODEL_INDEX = 128 - 14
        assert_eq!(
            d.pos_encoders.len(),
            (NUM_FULL_DISTANCES - END_POS_MODEL_INDEX) as usize
        );
        // align = 32 (4-bit tree)
        assert_eq!(d.align_encoder.len(), 1usize << (DIST_ALIGN_BITS + 1));
    }

    #[test]
    fn reset_restores_all_models() {
        let mut d = DistanceDecoder::with_default_states();
        for m in &mut d.slot_encoders {
            m.update(0);
        }
        for m in &mut d.pos_encoders {
            m.update(1);
        }
        d.reset_models();
        for m in &d.slot_encoders {
            assert_eq!(m.probability(), crate::constants::INIT_PROBS);
        }
        for m in &d.pos_encoders {
            assert_eq!(m.probability(), crate::constants::INIT_PROBS);
        }
    }

    #[test]
    #[should_panic(expected = "distance coder needs")]
    fn rejects_zero_states() {
        let _ = DistanceDecoder::new(0);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn rejects_len_state_overflow() {
        let mut d = DistanceDecoder::with_default_states();
        let mut rd = RangeDecoder::new(&[0u8; 32]).unwrap();
        let _ = d.decode(&mut rd, 99);
    }
}
