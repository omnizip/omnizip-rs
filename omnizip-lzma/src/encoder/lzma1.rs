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
use crate::constants::NUM_LEN_TO_POS_STATES;
use crate::encoder::match_finder::MatchFinder;
use crate::range_coder::RangeEncoder;
use crate::state::{LzmaState, NUM_STATES};

/// Minimum match length for LZMA (2 for rep matches, 3 for full matches).
const MATCH_LEN_MIN: u32 = 2;
const FULL_MATCH_LEN_MIN: u32 = 3;
/// Maximum encodable match length (LZMA spec limit).
const MATCH_LEN_MAX: u32 = 273;

/// LZMA1 encoder state — holds the probability models, range encoder,
/// and match-finder state.
#[derive(Debug)]
pub struct Lzma1Encoder {
    lc: u32,
    #[allow(dead_code)]
    lp: u32,
    pb: u32,
    pb_mask: u32,
    literal_mask: u32,
    state: LzmaState,
    is_match: Vec<BitModel>,
    is_rep: Vec<BitModel>,
    is_rep0: Vec<BitModel>,
    is_rep1: Vec<BitModel>,
    is_rep2: Vec<BitModel>,
    is_rep0_long: Vec<BitModel>,
    literal_encoder: LiteralEncoder,
    length_encoder: LengthEncoder,
    rep_length_encoder: LengthEncoder,
    distance_encoder: DistanceEncoder,
    range_encoder: RangeEncoder,
    rep0: u32,
    rep1: u32,
    rep2: u32,
    rep3: u32,
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
            is_rep0: vec![BitModel::new(); NUM_STATES],
            is_rep1: vec![BitModel::new(); NUM_STATES],
            is_rep2: vec![BitModel::new(); NUM_STATES],
            is_rep0_long: vec![BitModel::new(); NUM_STATES * pos_states],
            literal_encoder: LiteralEncoder::new(lit_capacity),
            length_encoder: LengthEncoder::new(pos_states),
            rep_length_encoder: LengthEncoder::new(pos_states),
            distance_encoder: DistanceEncoder::new(NUM_LEN_TO_POS_STATES as usize),
            range_encoder: RangeEncoder::new(),
            rep0: 0,
            rep1: 0,
            rep2: 0,
            rep3: 0,
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

    /// Lazy parser with explicit match-finder tuning knobs.
    ///
    /// `max_chain_length > 0` overrides the default chain depth;
    /// `nice_match > 0` stops the chain walk once a match this long
    /// is found. Pass 0 for either to use the encoder default.
    #[must_use]
    pub fn encode_with_tuning(
        self,
        input: &[u8],
        max_chain_length: u32,
        nice_match: u32,
    ) -> Vec<u8> {
        if max_chain_length == 0 && nice_match == 0 {
            return self.encode_with_parser(input, false);
        }
        if input.is_empty() {
            return self.encode_with_parser(input, false);
        }
        self.encode_via_lazy_tuned(input, max_chain_length, nice_match)
    }

    /// Optimal parser with explicit match-finder tuning knobs.
    #[must_use]
    pub fn encode_optimal_with_tuning(
        self,
        input: &[u8],
        max_chain_length: u32,
        nice_match: u32,
    ) -> Vec<u8> {
        if max_chain_length == 0 && nice_match == 0 {
            return self.encode_with_parser(input, true);
        }
        if input.is_empty() {
            return self.encode_with_parser(input, true);
        }
        self.encode_via_optimal_tuned(input, max_chain_length, nice_match)
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
                    self.encode_match(m1.distance, m1.length.min(MATCH_LEN_MAX), pos);
                    for _ in 0..m1.length.min(MATCH_LEN_MAX).saturating_sub(1) {
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

    /// Lazy parser with explicit match-finder tuning.
    fn encode_via_lazy_tuned(
        mut self,
        input: &[u8],
        max_chain_length: u32,
        nice_match: u32,
    ) -> Vec<u8> {
        let mut mf = MatchFinder::new(input, self.dict_size);
        if max_chain_length > 0 {
            mf.set_max_chain_length(max_chain_length);
        }
        if nice_match > 0 {
            mf.set_nice_match(nice_match);
        }

        while let Some(pos) = mf.advance() {
            let m1 = if pos + FULL_MATCH_LEN_MIN as usize <= input.len() {
                mf.find_match(pos)
            } else {
                None
            };

            if let Some(m1) = m1 {
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
                    self.encode_match(m1.distance, m1.length.min(MATCH_LEN_MAX), pos);
                    for _ in 0..m1.length.min(MATCH_LEN_MAX).saturating_sub(1) {
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

    /// Optimal parser with explicit match-finder tuning.
    fn encode_via_optimal_tuned(
        mut self,
        input: &[u8],
        max_chain_length: u32,
        nice_match: u32,
    ) -> Vec<u8> {
        use crate::encoder::optimal::{optimal_parse_actions_tuned, ParseAction};

        let actions =
            optimal_parse_actions_tuned(input, self.dict_size, max_chain_length, nice_match);

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
        if self.rep0 < pos as u32 {
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

        // Matched mode: state is a match context AND there is a previous
        // byte to reference. Must match the decoder's condition exactly
        // (decoder uses `is_match_context() && !output.is_empty()`).
        if self.state.is_match_context() && pos > 0 {
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

        // Rotate rep distances: rep3 ← rep2 ← rep1 ← rep0; rep0 = distance - 1.
        self.rep3 = self.rep2;
        self.rep2 = self.rep1;
        self.rep1 = self.rep0;
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
        // EOPM encodes length code 0 (length = MATCH_LEN_MIN), so
        // len_state = GetLenToPosState(MATCH_LEN_MIN) = 0. Using
        // pos_state here would select the wrong distance-slot models
        // and corrupt the stream.
        self.distance_encoder
            .encode(&mut self.range_encoder, 0xFFFF_FFFF, 0);
    }

    /// Emit a rep0 match (reuse the previous distance). The `length`
    /// is the actual match length (not length - MATCH_LEN_MIN).
    ///
    /// Bit layout (mirrors the decoder's `decode_rep_match`):
    /// 1. is_match = 1
    /// 2. is_rep = 1
    /// 3. is_rep0 = 0 (select rep0)
    /// 4. is_rep0_long = 1 (length > 1)
    /// 5. Length code (via the rep length coder's model set)
    fn encode_rep0_match(&mut self, length: u32, pos: usize) {
        let pos_state = (pos as u32) & self.pb_mask;
        let state_idx = usize::from(self.state.as_u8());
        let is_match_idx = state_idx * (1 << self.pb as usize) + pos_state as usize;

        // is_match = 1, is_rep = 1 (enters the rep-match branch).
        self.range_encoder
            .encode_bit(&mut self.is_match[is_match_idx], 1);
        self.range_encoder
            .encode_bit(&mut self.is_rep[state_idx], 1);

        // is_rep0 = 0 (select rep0 over rep1/2/3).
        self.range_encoder
            .encode_bit(&mut self.is_rep0[state_idx], 0);

        // is_rep0_long = 1 (long rep0, not short rep of length 1).
        let long_idx = state_idx * (1 << self.pb as usize) + pos_state as usize;
        self.range_encoder
            .encode_bit(&mut self.is_rep0_long[long_idx], 1);

        // Rep0 length coding (uses the rep length coder's model set,
        // which the decoder mirrors via `rep_length_coder`).
        let adjusted_len = length.saturating_sub(MATCH_LEN_MIN);
        self.rep_length_encoder
            .encode(&mut self.range_encoder, adjusted_len, pos_state as usize);

        self.state.on_rep();
    }

    /// Reset state and models (LZMA2 reset-state compatibility).
    pub fn reset_state(&mut self) {
        self.state = LzmaState::initial();
        self.rep0 = 0;
        self.rep1 = 0;
        self.rep2 = 0;
        self.rep3 = 0;
        for m in &mut self.is_match {
            m.reset();
        }
        for m in &mut self.is_rep {
            m.reset();
        }
        for m in &mut self.is_rep0 {
            m.reset();
        }
        for m in &mut self.is_rep1 {
            m.reset();
        }
        for m in &mut self.is_rep2 {
            m.reset();
        }
        for m in &mut self.is_rep0_long {
            m.reset();
        }
        self.literal_encoder.reset();
        self.length_encoder.reset_models();
        self.rep_length_encoder.reset_models();
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
        let out = dec
            .decode(&compressed, Some(input.len() as u64), true)
            .expect("decode");
        assert_eq!(out.as_slice(), input.as_ref());
    }

    #[test]
    fn repetitive_input_round_trips() {
        let input: Vec<u8> = b"abcdefgh".repeat(50);
        let enc = Lzma1Encoder::new(3, 0, 2);
        let compressed = enc.encode(&input);
        let mut dec = Lzma1Decoder::new(3, 0, 2, 1 << 16);
        let out = dec
            .decode(&compressed, Some(input.len() as u64), true)
            .expect("decode");
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
        let encode_once = || {
            Lzma1Encoder::new(3, 0, 2).encode(b"determinism test input with repetition repetition")
        };
        let a = encode_once();
        let b = encode_once();
        assert_eq!(a, b, "LZMA1 encoder non-deterministic");
    }

    #[test]
    fn tuned_encoder_is_deterministic() {
        let input: Vec<u8> = (0..4096)
            .map(|i| {
                if i % 100 < 50 {
                    (i % 26 + b'a' as i32) as u8
                } else {
                    (i % 256) as u8
                }
            })
            .collect();
        let encode = || Lzma1Encoder::new(3, 0, 2).encode_with_tuning(&input, 128, 64);
        let a = encode();
        let b = encode();
        assert_eq!(a, b, "tuned encoder non-deterministic");
    }

    #[test]
    fn tuned_optimal_is_deterministic() {
        let input: Vec<u8> = (0..4096)
            .map(|i| {
                if i % 100 < 50 {
                    (i % 26 + b'a' as i32) as u8
                } else {
                    (i % 256) as u8
                }
            })
            .collect();
        let encode = || Lzma1Encoder::new(3, 0, 2).encode_optimal_with_tuning(&input, 256, 128);
        let a = encode();
        let b = encode();
        assert_eq!(a, b, "tuned optimal encoder non-deterministic");
    }

    #[test]
    fn large_input_round_trips() {
        let input: Vec<u8> = (0..10_000)
            .map(|i| {
                if i % 100 < 50 {
                    (i % 26 + b'a' as i32) as u8
                } else {
                    (i % 256) as u8
                }
            })
            .collect();
        let enc = Lzma1Encoder::new(3, 0, 2);
        let compressed = enc.encode(&input);
        let mut dec = Lzma1Decoder::new(3, 0, 2, 1 << 16);
        let out = dec
            .decode(&compressed, Some(input.len() as u64), true)
            .expect("decode");
        assert_eq!(out.as_slice(), input.as_slice());
    }
}
