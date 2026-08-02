//! Length encoder — SDK length-coding scheme.
//!
//! Inverse of [`crate::coder::LengthDecoder`].
//!
//! ## Coding scheme
//!
//! - Lengths 0..=7 (raw): `choice=0`, then 3 bits via low tree.
//! - Lengths 8..=15: `choice=1`, `choice2=0`, then 3 bits via mid tree.
//! - Lengths 16..=271: `choice=1`, `choice2=1`, then 8 bits via high tree.

#![forbid(unsafe_code)]

use crate::bit_model::BitModel;
use crate::coder::decoder::encode_tree;
use crate::constants::{
    LEN_LOW_SYMBOLS, LEN_MID_SYMBOLS, NUM_LEN_HIGH_BITS, NUM_LEN_LOW_BITS, NUM_LEN_MID_BITS,
};
use crate::range_coder::RangeEncoder;

/// SDK-style length encoder. Mirrors [`crate::coder::LengthDecoder`].
#[derive(Debug)]
pub struct LengthEncoder {
    num_pos_states: usize,
    choice: BitModel,
    choice2: BitModel,
    low: Vec<BitModel>,
    mid: Vec<BitModel>,
    high: Vec<BitModel>,
}

impl LengthEncoder {
    /// Allocate a fresh encoder for `num_pos_states` position states.
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

    /// Reset every model to the initial probability.
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

    /// Encode a raw length value (callers subtract `MATCH_LEN_MIN`
    /// before passing).
    pub fn encode(&mut self, rc: &mut RangeEncoder, raw_length: u32, pos_state: usize) {
        assert!(pos_state < self.num_pos_states);
        let low_tree_size = 1usize << (NUM_LEN_LOW_BITS + 1);
        let mid_tree_size = 1usize << (NUM_LEN_MID_BITS + 1);

        if raw_length < LEN_LOW_SYMBOLS {
            rc.encode_bit(&mut self.choice, 0);
            let base = pos_state * low_tree_size;
            encode_tree(rc, &mut self.low[base..], NUM_LEN_LOW_BITS, raw_length);
        } else if raw_length < LEN_LOW_SYMBOLS + LEN_MID_SYMBOLS {
            rc.encode_bit(&mut self.choice, 1);
            rc.encode_bit(&mut self.choice2, 0);
            let base = pos_state * mid_tree_size;
            encode_tree(
                rc,
                &mut self.mid[base..],
                NUM_LEN_MID_BITS,
                raw_length - LEN_LOW_SYMBOLS,
            );
        } else {
            rc.encode_bit(&mut self.choice, 1);
            rc.encode_bit(&mut self.choice2, 1);
            encode_tree(
                rc,
                &mut self.high,
                NUM_LEN_HIGH_BITS,
                raw_length - LEN_LOW_SYMBOLS - LEN_MID_SYMBOLS,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coder::LengthDecoder;
    use crate::range_coder::RangeDecoder;

    #[test]
    fn round_trip_low_length() {
        let mut enc = LengthEncoder::new(2);
        let mut rc = RangeEncoder::new();
        enc.encode(&mut rc, 3, 0);
        rc.flush();
        let bytes = rc.finish();
        let mut dec_rc = RangeDecoder::new(&bytes).expect("init");
        let mut dec = LengthDecoder::new(2);
        let recovered = dec.decode(&mut dec_rc, 0).expect("decode");
        assert_eq!(recovered, 3);
    }

    #[test]
    fn round_trip_mid_length() {
        let mut enc = LengthEncoder::new(2);
        let mut rc = RangeEncoder::new();
        enc.encode(&mut rc, 10, 0);
        rc.flush();
        let bytes = rc.finish();
        let mut dec_rc = RangeDecoder::new(&bytes).expect("init");
        let mut dec = LengthDecoder::new(2);
        let recovered = dec.decode(&mut dec_rc, 0).expect("decode");
        assert_eq!(recovered, 10);
    }

    #[test]
    fn round_trip_high_length() {
        let mut enc = LengthEncoder::new(2);
        let mut rc = RangeEncoder::new();
        enc.encode(&mut rc, 200, 1);
        rc.flush();
        let bytes = rc.finish();
        let mut dec_rc = RangeDecoder::new(&bytes).expect("init");
        let mut dec = LengthDecoder::new(2);
        let recovered = dec.decode(&mut dec_rc, 1).expect("decode");
        assert_eq!(recovered, 200);
    }
}
