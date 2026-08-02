//! Distance encoder — SDK distance-coding scheme.
//!
//! Inverse of [`crate::coder::DistanceDecoder`]. Phase B: supports
//! slots 0..=13 (short distances) and the align-tree path for slots
//! ≥14. The direct-bits path for slots ≥14 is incomplete — full
//! EOPM encoding requires the exact bit-by-bit reconstruction used
//! by `decode_direct_bits_with_base`. Tracked in TODO.complete/13.

#![forbid(unsafe_code)]

use crate::bit_model::BitModel;
use crate::coder::decoder::{encode_reverse_tree, encode_tree};
use crate::constants::{
    DIST_ALIGN_BITS, END_POS_MODEL_INDEX, NUM_DIST_SLOT_BITS, NUM_FULL_DISTANCES,
    START_POS_MODEL_INDEX,
};
use crate::range_coder::RangeEncoder;

const DEFAULT_NUM_LEN_TO_POS_STATES: usize = crate::constants::NUM_LEN_TO_POS_STATES as usize;

/// SDK distance encoder.
#[derive(Debug)]
pub struct DistanceEncoder {
    slot_encoders: Vec<BitModel>,
    pos_encoders: Vec<BitModel>,
    align_encoder: Vec<BitModel>,
    num_len_to_pos_states: usize,
}

impl DistanceEncoder {
    #[must_use]
    pub fn new(num_len_to_pos_states: usize) -> Self {
        assert!(num_len_to_pos_states > 0);
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

    #[must_use]
    pub fn with_default_states() -> Self {
        Self::new(DEFAULT_NUM_LEN_TO_POS_STATES)
    }

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

    /// Encode a distance (value before adding 1; 0 → distance 1).
    pub fn encode(&mut self, rc: &mut RangeEncoder, distance: u32, len_state: usize) {
        assert!(len_state < self.num_len_to_pos_states);
        let slot_tree_size = 1usize << (NUM_DIST_SLOT_BITS + 1);
        let base = len_state * slot_tree_size;

        let slot = distance_slot(distance);
        encode_tree(rc, &mut self.slot_encoders[base..], NUM_DIST_SLOT_BITS, slot);

        if slot < START_POS_MODEL_INDEX {
            return;
        }

        let footer_bits = (slot >> 1) - 1;

        if slot < END_POS_MODEL_INDEX {
            let slot_base = (2 | (slot & 1)) << footer_bits;
            let pos_idx = i64::from(slot_base) - i64::from(slot) - 1;
            let extra = distance - slot_base;
            encode_reverse_tree(rc, &mut self.pos_encoders, pos_idx, footer_bits, extra);
        } else {
            // Slots ≥14: high direct bits + low aligned bits.
            //
            // Decoder logic (`decode_direct_bits_with_base`):
            //   result = base
            //   for each bit:
            //     result = (result << 1) + 1   // tentative 1
            //     range >>= 1
            //     bit = (code >= range) ? 1 : 0
            //     if bit == 1: code -= range; result keeps the +1.
            //     if bit == 0: result -= 1     // change to 0
            //
            // For the encoder to produce `bit == 1`, the decoder's `code`
            // must be in the HIGH half of [0, range). So the encoder
            // narrows its interval to the HIGH half: `low += range` AFTER
            // halving (the half goes into the high position).
            // For `bit == 0`, the encoder narrows to the LOW half: do
            // nothing (low stays, range halves).
            let num_direct_bits = footer_bits - DIST_ALIGN_BITS;
            let low_mask = (1u32 << DIST_ALIGN_BITS) - 1;
            let direct_value = (distance - (2 + (slot & 1))) >> DIST_ALIGN_BITS;

            // Emit direct bits MSB-first.
            for i in (0..num_direct_bits).rev() {
                let bit = (direct_value >> i) & 1;
                rc.normalize();
                rc.range_div2();
                if bit == 1 {
                    // Encoder narrows to high half: low += range (new halved range).
                    rc.add_range();
                }
                // bit == 0: encoder narrows to low half (do nothing).
            }

            let low_bits = distance & low_mask;
            encode_reverse_tree(rc, &mut self.align_encoder, 0, DIST_ALIGN_BITS, low_bits);
        }
    }
}

/// Compute the LZMA distance slot. Matches `get_pos_slot` in XZ Utils.
fn distance_slot(distance: u32) -> u32 {
    if distance < 4 {
        return distance;
    }
    let high_bit = distance.ilog2();
    (high_bit << 1) + ((distance >> (high_bit - 1)) & 1) - 2
}
