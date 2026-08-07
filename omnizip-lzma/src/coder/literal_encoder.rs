//! Literal byte encoder.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/lzma/literal_encoder.rb`
//! (208 LOC, MIT, Ribose Inc.) — the inverse of [`crate::coder::LiteralDecoder`].
//!
//! LZMA literals are context-coded byte values: each bit of the byte
//! uses a [`BitModel`] selected by the partial symbol value. When the
//! previous packet was a match, a "matched literal" mode adds the
//! byte at the last match distance as additional context.

#![forbid(unsafe_code)]

use crate::bit_model::BitModel;
use crate::range_coder::RangeEncoder;

/// 0x100 — the sentinel at which the bit-tree walk stops.
const SYMBOL_DONE: u32 = 0x100;

/// Literal sub-coder. Stateless aside from the probability model array.
#[derive(Debug)]
pub struct LiteralEncoder {
    models: Vec<BitModel>,
}

impl LiteralEncoder {
    /// Allocate a literal coder with `cap` probability slots.
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            models: vec![BitModel::new(); cap],
        }
    }

    /// Read-only access to the underlying model array.
    #[must_use]
    pub fn models(&self) -> &[BitModel] {
        &self.models
    }

    /// Reset every model to the initial probability.
    pub fn reset(&mut self) {
        for m in &mut self.models {
            m.reset();
        }
    }

    /// Encode a byte in unmatched (literal-context-only) mode.
    ///
    /// `lit_state` is the unshifted context value; `lc` is the
    /// literal-context-bits parameter from the LZMA header.
    pub fn encode_unmatched(
        &mut self,
        byte: u8,
        lit_state: u32,
        lc: u32,
        rc: &mut RangeEncoder,
    ) {
        let base_offset = 3 * (lit_state << lc);
        // Walk the bit tree top-down. The encoder starts symbol at 1 and
        // emits the high bit of `byte` first (after prepending an implicit
        // 1 bit). For each bit, the model index is `base_offset + symbol`.
        let mut symbol = 1u32;
        // `byte` occupies bits 0-7; the bit-tree walk emits bit 7 first.
        for i in (0..8).rev() {
            let bit = u32::from((byte >> i) & 1);
            let idx = (base_offset + symbol) as usize;
            rc.encode_bit(&mut self.models[idx], bit);
            symbol = (symbol << 1) | bit;
        }
        debug_assert_eq!(symbol, SYMBOL_DONE + u32::from(byte));
    }

    /// Encode a byte in matched mode (previous packet was a match).
    /// `match_byte` is the byte at `rep0` distance back in the output.
    #[allow(clippy::too_many_lines)]
    pub fn encode_matched(
        &mut self,
        byte: u8,
        match_byte: u8,
        lit_state: u32,
        lc: u32,
        rc: &mut RangeEncoder,
    ) {
        let base_offset = 3 * (lit_state << lc);
        let mut symbol = 1u32;
        let mut match_sym = u32::from(match_byte);
        let mut offset = SYMBOL_DONE;

        loop {
            match_sym <<= 1;
            let match_bit = match_sym & offset;

            // The bit of `byte` at this tree depth. symbol=1 → depth 0
            // → bit 7 (MSB); symbol=2..3 → depth 1 → bit 6; etc.
            let depth = symbol.ilog2() as usize;
            let bit = if depth < 8 {
                u32::from((byte >> (7 - depth)) & 1)
            } else {
                0
            };

            let model_idx = (base_offset + offset + match_bit + symbol) as usize;
            rc.encode_bit(&mut self.models[model_idx], bit);

            if bit == 0 {
                offset &= !match_bit;
                symbol <<= 1;
            } else {
                offset &= match_bit;
                symbol = (symbol << 1) | 1;
            }

            let match_bit_flag = u32::from(match_bit > 0);
            if match_bit_flag != bit {
                // Switch to unmatched mode for the remaining bits.
                if symbol < SYMBOL_DONE {
                    self.encode_unmatched_tail(byte, symbol, base_offset, rc);
                }
                break;
            }

            if symbol >= SYMBOL_DONE {
                break;
            }
        }
    }

    /// Continue encoding in unmatched mode from a partial `symbol`.
    fn encode_unmatched_tail(
        &mut self,
        byte: u8,
        start_symbol: u32,
        base_offset: u32,
        rc: &mut RangeEncoder,
    ) {
        let mut symbol = start_symbol;
        while symbol < SYMBOL_DONE {
            let depth = symbol.ilog2() as usize;
            let bit_pos = 7 - depth as i32;
            let bit = if bit_pos >= 0 {
                u32::from((byte >> bit_pos) & 1)
            } else {
                0
            };
            let idx = (base_offset + symbol) as usize;
            rc.encode_bit(&mut self.models[idx], bit);
            symbol = (symbol << 1) | bit;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coder::LiteralDecoder;
    use crate::range_coder::RangeDecoder;

    #[test]
    fn allocates_with_given_capacity() {
        let e = LiteralEncoder::new(64);
        assert_eq!(e.models().len(), 64);
    }

    #[test]
    fn unmatched_round_trips() {
        // Use small lc/lp/pb for the round-trip.
        let lc = 3;
        let lit_state = 0x12;
        let byte = 0x42;
        let mut enc = LiteralEncoder::new(0x300);
        let mut rc = RangeEncoder::new();
        enc.encode_unmatched(byte, lit_state, lc, &mut rc);
        rc.flush();
        let bytes = rc.finish();
        let mut dec = RangeDecoder::new(&bytes).expect("init");
        let mut dec_lit = LiteralDecoder::new(0x300);
        let recovered = dec_lit.decode_unmatched(lit_state, lc, &mut dec).expect("decode");
        assert_eq!(recovered, byte);
    }

    #[test]
    fn determinism_same_byte_same_output() {
        let encode_once = || {
            let mut e = LiteralEncoder::new(0x300);
            let mut rc = RangeEncoder::new();
            for b in b"hello world" {
                e.encode_unmatched(*b, 0, 3, &mut rc);
            }
            rc.flush();
            rc.finish()
        };
        let a = encode_once();
        let b = encode_once();
        assert_eq!(a, b, "literal encoder non-deterministic");
    }
}
