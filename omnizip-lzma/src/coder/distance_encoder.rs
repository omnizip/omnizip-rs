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
        encode_tree(
            rc,
            &mut self.slot_encoders[base..],
            NUM_DIST_SLOT_BITS,
            slot,
        );

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
            // Matches XZ Utils `lzma_encoder.c::match()`:
            //   base = (2 | (slot & 1)) << footer_bits
            //   dist_reduced = distance - base
            //   rc_direct(dist_reduced >> ALIGN_BITS, footer_bits - ALIGN_BITS)
            //   rc_bittree_reverse(dist_align, ALIGN_BITS,
            //                      dist_reduced & ALIGN_MASK)
            //
            // The decoder's `decode_direct_bits_with_base` starts from
            // `result = 2 + (slot & 1)` and doubles+1 each step. The
            // encoder must emit the direct bits of `dist_reduced >>
            // ALIGN_BITS` so the decoder reconstructs the correct value.
            //
            // BUG HISTORY: an earlier version used
            //   direct_value = (distance - (2 + (slot & 1))) >> DIST_ALIGN_BITS
            // and
            //   low_bits = distance & low_mask
            // instead of subtracting `base` and masking `dist_reduced`.
            // For small distances the two formulas agree (footer_bits
            // is small), but for slot 63 (EOPM, distance = 0xFFFFFFFF)
            // the wrong formula produced direct_value = 0x0FFFFFFF and
            // low_bits = 0xF, while the correct values are
            // direct_value = 0x03FFFFFF and low_bits = 0xF. The
            // decoder then reconstructed the wrong distance and the
            // `rep0 == UINT32_MAX` check failed, causing xz-utils to
            // report "Compressed data is corrupt".
            let num_direct_bits = footer_bits - DIST_ALIGN_BITS;
            let low_mask = (1u32 << DIST_ALIGN_BITS) - 1;
            let dist_base = (2 | (slot & 1)) << footer_bits;
            let dist_reduced = distance - dist_base;
            let direct_value = dist_reduced >> DIST_ALIGN_BITS;

            // Emit direct bits MSB-first. The decoder mirrors this by
            // starting from `result = 2 + (slot & 1)` and applying
            // `(result << 1) + 1` then adjusting on bit == 0.
            for i in (0..num_direct_bits).rev() {
                let bit = (direct_value >> i) & 1;
                rc.normalize();
                rc.range_div2();
                if bit == 1 {
                    // Encoder narrows to high half: low += range (halved).
                    rc.add_range();
                }
                // bit == 0: encoder narrows to low half (do nothing).
            }

            let low_bits = dist_reduced & low_mask;
            encode_reverse_tree(rc, &mut self.align_encoder, 0, DIST_ALIGN_BITS, low_bits);
        }
    }
}

/// Compute the LZMA distance slot. Matches `get_dist_slot` in XZ Utils
/// (`src/liblzma/lzma/fastpos.h`) and `get_pos_slot` in the LZMA SDK.
///
/// `distance` is the 0-based form (0 = "1 byte back"). Slots 0..=3 are
/// identity with the distance; for distance ≥ 4 the slot encodes the
/// position of the highest set bit plus the bit just below it.
///
/// The formula `(high_bit << 1) + ((distance >> (high_bit - 1)) & 1)`
/// matches the C reference `get_dist_slot_2`. Note: an earlier version of
/// this function subtracted 2 from the result and used `distance < 4`
/// instead of `distance <= 4`. Both bugs were invisible to the Rust-only
/// round-trip tests (the decoder walks the bit-tree from the stream and
/// never recomputes the slot), but broke interop with `xz`/`lzma`:
///
/// - The `< 4` guard vs `<= 4` only matters at `distance == 4`, where
///   both paths happen to return 4, so this was harmless in practice
///   but fixed for spec-conformance.
/// - The `- 2` meant every distance ≥ 4 produced a slot value 2 too
///   low, so the EOPM marker `distance = 0xFFFFFFFF` produced slot 61
///   instead of 63. The decoder then reconstructed `rep0 = 0x7FFFFFFF`
///   instead of `0xFFFFFFFF` and never entered the EOPM branch,
///   causing xz-utils to report "Compressed data is corrupt".
fn distance_slot(distance: u32) -> u32 {
    // Matches the C `dist <= 4` early-return path: get_dist_slot(4) = 4.
    if distance <= 4 {
        return distance;
    }
    let high_bit = distance.ilog2();
    (high_bit << 1) + ((distance >> (high_bit - 1)) & 1)
}
