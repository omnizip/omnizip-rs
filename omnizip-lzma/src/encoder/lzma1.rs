//! LZMA1 packet encoder with match-finder integration and lazy parsing.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/lzma/encoder.rb`.
//!
//! Uses a hash-chain match finder to find LZ77 matches, then encodes
//! them with the LZMA probability models (literal, length, distance)
//! via the range encoder. Supports both unmatched and matched literal
//! contexts.

#![forbid(unsafe_code)]

use crate::bit_model::BitModel;
use crate::coder::{DistanceEncoder, LengthEncoder, LiteralEncoder};
use crate::encoder::match_finder::MatchFinder;
use crate::range_coder::RangeEncoder;
use crate::state::{LzmaState, NUM_STATES};
use crate::constants::NUM_LEN_TO_POS_STATES;

/// Minimum match length for LZMA (2 for rep matches, 3 for full matches).
const MATCH_LEN_MIN: u32 = 2;
const FULL_MATCH_LEN_MIN: u32 = 3;

/// LZMA1 encoder state — holds the probability models, range encoder,
/// and match-finder state.
#[derive(Debug)]
pub struct Lzma1Encoder {
    lc: u32,
    #[allow(dead_code)] lp: u32,
    pb: u32,
    pb_mask: u32,
    literal_mask: u32,
    state: LzmaState,
    is_match: Vec<BitModel>,
    is_rep: Vec<BitModel>,
    literal_encoder: LiteralEncoder,
    length_encoder: LengthEncoder,
    distance_encoder: DistanceEncoder,
    range_encoder: RangeEncoder,
    rep0: u32,
    dict_size: u32,
}

impl Lzma1Encoder {
    /// Construct an encoder for the given LZMA parameters.
    #[must_use]
    #[allow(clippy::similar_names)]
    pub fn new(lc: u32, #[allow(dead_code)] lp: u32, pb: u32) -> Self {
        Self::with_dict_size(lc, lp, pb, 1 << 24)
    }

    /// Construct with a specific dictionary size (for match finder).
    #[must_use]
    #[allow(clippy::similar_names)]
    pub fn with_dict_size(lc: u32, #[allow(dead_code)] lp: u32, pb: u32, dict_size: u32) -> Self {
        assert!(lc <= 8, "lc must be 0..=8");
        assert!(lp <= 4, "lp must be 0..=4");
        assert!(pb <= 4, "pb must be 0..=4");
        assert!(lc + lp <= 4, "lc + lp must be ≤ 4");

        let pos_states = 1usize << pb;
        let pb_mask = pos_states as u32 - 1;
        let literal_mask = (0x100u32 << lp).wrapping_sub(0x100u32 >> lc);
        let lit_capacity = ((literal_mask * 3) << lc) as usize + 0x300 + 1;

        Self {
            lc,
            lp,
            pb,
            pb_mask,
            literal_mask,
            state: LzmaState::initial(),
            is_match: vec![BitModel::new(); NUM_STATES * pos_states],
            is_rep: vec![BitModel::new(); NUM_STATES],
            literal_encoder: LiteralEncoder::new(lit_capacity),
            length_encoder: LengthEncoder::new(pos_states),
            distance_encoder: DistanceEncoder::new(NUM_LEN_TO_POS_STATES as usize),
            range_encoder: RangeEncoder::new(),
            rep0: 0,
            dict_size,
        }
    }

    /// Encode `input` as an LZMA1 stream with lazy (look-ahead-1)
    /// parsing. At each position, checks if deferring the match by
    /// one byte yields a longer match. If so, emits a literal and
    /// takes the deferred match; otherwise takes the current match.
    #[must_use]
    pub fn encode(self, input: &[u8]) -> Vec<u8> {
        self.encode_with_parser(input, false)
    }

    /// Encode using the optimal (DP) parser. Gives 3-8% better ratio
    /// than `encode` at the cost of O(n) DP computation.
    #[must_use]
    pub fn encode_optimal(self, input: &[u8]) -> Vec<u8> {
        self.encode_with_parser(input, true)
    }

    /// Internal encode: dispatches between lazy and optimal parsing.
    fn encode_with_parser(mut self, input: &[u8], use_optimal: bool) -> Vec<u8> {
        if input.is_empty() {
            self.encode_eopm(0);
            self.range_encoder.flush();
            return self.range_encoder.finish();
        }

        if use_optimal {
            self.encode_via_optimal(input)
        } else {
            self.encode_via_lazy(input)
        }
    }

    /// Lazy parser (look-ahead-1).
    fn encode_via_lazy(mut self, input: &[u8]) -> Vec<u8> {
        let mut mf = MatchFinder::new(input, self.dict_size);

        while let Some(pos) = mf.advance() {
            let m1 = if pos + FULL_MATCH_LEN_MIN as usize <= input.len() {
                mf.find_match(pos)
            } else {
                None
            };

            if let Some(m1) = m1 {
                // Lazy: check if position+1 has a better match.
                let better_at_next = if pos + 1 < input.len() {
                    let m2 = if pos + 1 + FULL_MATCH_LEN_MIN as usize <= input.len() {
                        mf.find_match(pos + 1)
                    } else {
                        None
                    };
                    match m2 {
                        Some(m2) => m2.length > m1.length + 1,
                        None => false,
                    }
                } else {
                    false
                };

                if better_at_next {
                    let prev_byte = if pos > 0 { input[pos - 1] } else { 0 };
                    let match_byte = self.get_match_byte(input, pos);
                    self.encode_literal_byte(input[pos], prev_byte, match_byte, pos);
                } else {
                    self.encode_match(m1.distance, m1.length, pos);
                    for _ in 0..m1.length.saturating_sub(1) {
                        if mf.advance().is_none() {
                            break;
                        }
                    }
                }
            } else {
                let prev_byte = if pos > 0 { input[pos - 1] } else { 0 };
                let match_byte = self.get_match_byte(input, pos);
                self.encode_literal_byte(input[pos], prev_byte, match_byte, pos);
            }
        }

        self.encode_eopm(input.len());
        self.range_encoder.flush();
        self.range_encoder.finish()
    }

    /// Optimal parser: use the DP-based parse planner, then emit.
    fn encode_via_optimal(mut self, input: &[u8]) -> Vec<u8> {
        use crate::encoder::optimal::{optimal_parse_actions, ParseAction};

        let actions = optimal_parse_actions(input, self.dict_size);

        for (pos, action) in actions {
            match action {
                ParseAction::Literal(byte) => {
                    let prev_byte = if pos > 0 { input[pos - 1] } else { 0 };
                    let match_byte = self.get_match_byte(input, pos);
                    self.encode_literal_byte(byte, prev_byte, match_byte, pos);
                }
                ParseAction::Match { distance, length } => {
                    self.encode_match(distance, length, pos);
                }
                ParseAction::Rep0Match { length } => {
                    // Encode a rep0 match. rep0 distance is already set
                    // from the last encode_match call.
                    self.encode_rep0_match(length, pos);
                }
            }
        }

        self.encode_eopm(input.len());
        self.range_encoder.flush();
        self.range_encoder.finish()
    }

    /// Get the byte at rep0 distance back (for matched-literal context).
    fn get_match_byte(&self, input: &[u8], pos: usize) -> u8 {
        if self.rep0 > 0 && self.rep0 < pos as u32 {
            input[pos - self.rep0 as usize - 1]
        } else {
            0
        }
    }

    /// Emit a literal byte packet with context-appropriate encoding.
    fn encode_literal_byte(&mut self, byte: u8, prev_byte: u8, match_byte: u8, pos: usize) {
        let pos_state = (pos as u32) & self.pb_mask;
        let state_idx = usize::from(self.state.as_u8());
        let is_match_idx = state_idx * (1 << self.pb as usize) + pos_state as usize;

        // is_match = 0 → literal packet.
        self.range_encoder
            .encode_bit(&mut self.is_match[is_match_idx], 0);

        let lit_state = ((pos as u32) << 8 | u32::from(prev_byte)) & self.literal_mask;

        if self.state.is_match_context() && self.rep0 > 0 {
            self.literal_encoder.encode_matched(
                byte,
                match_byte,
                lit_state,
                self.lc,
                &mut self.range_encoder,
            );
        } else {
            self.literal_encoder.encode_unmatched(
                byte,
                lit_state,
                self.lc,
                &mut self.range_encoder,
            );
        }

        self.state.on_literal();
    }

    /// Emit a match packet (is_match=1, is_rep=0, length, distance).
    fn encode_match(&mut self, distance: u32, length: u32, pos: usize) {
        let pos_state = (pos as u32) & self.pb_mask;
        let state_idx = usize::from(self.state.as_u8());
        let is_match_idx = state_idx * (1 << self.pb as usize) + pos_state as usize;

        // is_match = 1.
        self.range_encoder
            .encode_bit(&mut self.is_match[is_match_idx], 1);

        // is_rep = 0 → new distance.
        self.range_encoder
            .encode_bit(&mut self.is_rep[state_idx], 0);

        // Length code.
        let len_code = length.saturating_sub(MATCH_LEN_MIN);
        self.length_encoder
            .encode(&mut self.range_encoder, len_code, pos_state as usize);

        // Distance (0-based: match finder returns 1-based, encoder wants 0-based).
        let len_state = std::cmp::min(
            usize::try_from(len_code).unwrap_or(0),
            NUM_LEN_TO_POS_STATES as usize - 1,
        );
        self.distance_encoder
            .encode(&mut self.range_encoder, distance - 1, len_state);

        self.rep0 = distance - 1;
        self.state.on_match();
    }

    /// Emit the LZMA End-of-Payload-Marker.
    fn encode_eopm(&mut self, pos: usize) {
        let pos_state = (pos as u32) & self.pb_mask;
        let state_idx = usize::from(self.state.as_u8());
        let is_match_idx = state_idx * (1 << self.pb as usize) + pos_state as usize;

        self.range_encoder
            .encode_bit(&mut self.is_match[is_match_idx], 1);
        self.range_encoder
            .encode_bit(&mut self.is_rep[state_idx], 0);
        self.length_encoder
            .encode(&mut self.range_encoder, 0, pos_state as usize);
        let len_state = std::cmp::min(pos_state as usize, NUM_LEN_TO_POS_STATES as usize - 1);
        self.distance_encoder
            .encode(&mut self.range_encoder, 0xFFFF_FFFF, len_state);
    }

    /// Emit a rep0 match (reuse the previous distance). The `length`
    /// is the actual match length (not length - MATCH_LEN_MIN).
    fn encode_rep0_match(&mut self, length: u32, pos: usize) {
        let pos_state = (pos as u32) & self.pb_mask;
        let state_idx = usize::from(self.state.as_u8());
        let is_match_idx = state_idx * (1 << self.pb as usize) + pos_state as usize;

        self.range_encoder
            .encode_bit(&mut self.is_match[is_match_idx], 1);
        self.range_encoder
            .encode_bit(&mut self.is_rep[state_idx], 1);

        // Rep0 length coding (no distance encoding needed).
        let adjusted_len = length.saturating_sub(MATCH_LEN_MIN);
        self.length_encoder
            .encode(&mut self.range_encoder, adjusted_len, pos_state as usize);

        self.state.on_match();
    }

    /// Reset state and models (LZMA2 reset-state compatibility).
    pub fn reset_state(&mut self) {
        self.state = LzmaState::initial();
        self.rep0 = 0;
        for m in &mut self.is_match {
            m.reset();
        }
        self.literal_encoder.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::Lzma1Decoder;

    #[test]
    fn empty_input_round_trips() {
        let enc = Lzma1Encoder::new(3, 0, 2);
        let compressed = enc.encode(&[]);
        let mut dec = Lzma1Decoder::new(3, 0, 2, 1 << 16);
        let out = dec.decode(&compressed, Some(0), true).expect("decode");
        assert!(out.is_empty());
    }

    #[test]
    fn small_input_round_trips() {
        let input = b"hello world";
        let enc = Lzma1Encoder::new(3, 0, 2);
        let compressed = enc.encode(input);
        let mut dec = Lzma1Decoder::new(3, 0, 2, 1 << 16);
        let out = dec.decode(&compressed, Some(input.len() as u64), true).expect("decode");
        assert_eq!(out.as_slice(), input.as_ref());
    }

    #[test]
    fn repetitive_input_round_trips() {
        let input: Vec<u8> = b"abcdefgh".repeat(50);
        let enc = Lzma1Encoder::new(3, 0, 2);
        let compressed = enc.encode(&input);
        let mut dec = Lzma1Decoder::new(3, 0, 2, 1 << 16);
        let out = dec.decode(&compressed, Some(input.len() as u64), true).expect("decode");
        assert_eq!(out.as_slice(), input.as_slice());
    }

    #[test]
    fn repetitive_input_compresses() {
        let input: Vec<u8> = b"the quick brown fox ".repeat(100);
        let enc = Lzma1Encoder::new(3, 0, 2);
        let compressed = enc.encode(&input);
        assert!(
            compressed.len() < input.len(),
            "LZMA should compress repetitive input: {} vs {}",
            compressed.len(),
            input.len()
        );
    }

    #[test]
    fn determinism_same_input_same_output() {
        let encode_once = || Lzma1Encoder::new(3, 0, 2).encode(b"determinism test input with repetition repetition");
        let a = encode_once();
        let b = encode_once();
        assert_eq!(a, b, "LZMA1 encoder non-deterministic");
    }

    #[test]
    fn large_input_round_trips() {
        let input: Vec<u8> = (0..10_000)
            .map(|i| if i % 100 < 50 { (i % 26 + b'a' as i32) as u8 } else { (i % 256) as u8 })
            .collect();
        let enc = Lzma1Encoder::new(3, 0, 2);
        let compressed = enc.encode(&input);
        let mut dec = Lzma1Decoder::new(3, 0, 2, 1 << 16);
        let out = dec.decode(&compressed, Some(input.len() as u64), true).expect("decode");
        assert_eq!(out.as_slice(), input.as_slice());
    }
}
