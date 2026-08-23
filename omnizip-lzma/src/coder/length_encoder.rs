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
    /// Cached symbol prices per position state (port of
    /// `lzma_length_encoder.prices`), indexed by `len - MATCH_LEN_MIN`.
    prices: Vec<u32>,
    /// Number of priced symbols (port of `lzma_length_encoder.table_size`).
    table_size: u32,
    /// Refresh countdowns per position state (port of
    /// `lzma_length_encoder.counters`).
    counters: Vec<u32>,
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
            prices: vec![0; num_pos_states * 272],
            table_size: 0,
            counters: vec![0; num_pos_states],
        }
    }

    /// Reset every model to the initial probability.
    /// Port of `length_update_prices()` in xz's lzma_encoder.c:
    /// recompute the cached price table for `pos_state` from the
    /// current choice/tree models.
    pub fn update_prices(&mut self, pos_state: usize) {
        use crate::range_coder::price::{rc_bit_0_price, rc_bit_1_price, rc_bittree_price};

        let table_size = self.table_size as usize;
        self.counters[pos_state] = self.table_size;

        let a0 = rc_bit_0_price(self.choice.probability());
        let a1 = rc_bit_1_price(self.choice.probability());
        let b0 = a1 + rc_bit_0_price(self.choice2.probability());
        let b1 = a1 + rc_bit_1_price(self.choice2.probability());

        let low_tree_size = 1usize << (NUM_LEN_LOW_BITS + 1);
        let mid_tree_size = 1usize << (NUM_LEN_MID_BITS + 1);
        let low = &self.low[pos_state * low_tree_size..(pos_state + 1) * low_tree_size];
        let mid = &self.mid[pos_state * mid_tree_size..(pos_state + 1) * mid_tree_size];

        let mut i = 0usize;
        while i < table_size && i < LEN_LOW_SYMBOLS as usize {
            self.prices[pos_state * 272 + i] =
                a0 + rc_bittree_price(low, NUM_LEN_LOW_BITS, i as u32);
            i += 1;
        }
        while i < table_size && i < (LEN_LOW_SYMBOLS + LEN_MID_SYMBOLS) as usize {
            self.prices[pos_state * 272 + i] =
                b0 + rc_bittree_price(mid, NUM_LEN_MID_BITS, (i as u32) - LEN_LOW_SYMBOLS);
            i += 1;
        }
        while i < table_size {
            self.prices[pos_state * 272 + i] = b1
                + rc_bittree_price(
                    &self.high,
                    NUM_LEN_HIGH_BITS,
                    (i as u32) - LEN_LOW_SYMBOLS - LEN_MID_SYMBOLS,
                );
            i += 1;
        }
    }

    /// Set the number of priced length symbols (`table_size`).
    pub fn set_table_size(&mut self, nice_len: u32) {
        // Port of the length-encoder init in lzma_encoder.c:
        // table_size = nice_len + 1 - MATCH_LEN_MIN. hash bytes for
        // HC4 is 4, so the effective nice_len never drops below 4.
        self.table_size = nice_len.max(4) + 1 - 2;
    }

    /// Cached price for raw length code `len_code` at `pos_state`.
    #[must_use]
    pub fn price(&self, len_code: u32, pos_state: usize) -> u32 {
        self.prices[pos_state * 272 + len_code as usize]
    }

    /// Port of the counter decrement in `length()`: refresh the price
    /// table when the countdown hits zero.
    pub fn note_encoded(&mut self, pos_state: usize) {
        self.counters[pos_state] = self.counters[pos_state].saturating_sub(1);
        if self.counters[pos_state] == 0 && self.table_size > 0 {
            self.update_prices(pos_state);
        }
    }

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
