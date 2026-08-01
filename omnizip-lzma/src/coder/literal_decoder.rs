//! Literal byte decoder.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/lzma/literal_decoder.rb`
//! (204 LOC, MIT, Ribose Inc.).
//!
//! LZMA literals are context-coded byte values: each bit of the byte
//! uses a [`BitModel`] selected by the partial symbol value. When the
//! decoder is in a match context (the previous packet was a match), it
//! uses a "matched literal" mode where the byte at the last match
//! distance provides additional context.
//!
//! ## XZ Utils formula
//!
//! `base_offset = 3 * (lit_state << lc)`. The shift happens *before* the
//! triple — match the Ruby order, or the model indices drift.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::bit_model::BitModel;
use crate::LzmaError;
use crate::RangeDecoder;

/// Literal sub-coder. Stateless aside from the probability model array;
/// one instance per LZMA stream. Methods mirror the Ruby's
/// `decode_unmatched` and `decode_matched`.
#[derive(Debug)]
pub struct LiteralDecoder {
    models: Vec<BitModel>,
}

/// 0x100 — the sentinel at which the bit-tree walk stops. Reaching it
/// means all 8 literal bits have been decoded.
const SYMBOL_DONE: u32 = 0x100;

impl LiteralDecoder {
    /// Allocate a literal coder with `cap` probability slots. Callers
    /// pass the LZMA parameters' literal-context-table size; the Ruby
    /// does this implicitly via Ruby's lazy `||=` initialisation.
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            models: vec![BitModel::new(); cap],
        }
    }

    /// Read-only access to the underlying model array. Used by tests
    /// and by encoder variants that share the same model pool.
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

    /// Decode a byte in unmatched (literal-context-only) mode.
    ///
    /// `lit_state` is the unshifted context value (typically
    /// `((position << 8) + prev_byte) & literal_mask`); `lc` is the
    /// literal-context-bits parameter from the LZMA header.
    ///
    /// # Errors
    ///
    /// Forwards any [`LzmaError`] from the range decoder.
    pub fn decode_unmatched(
        &mut self,
        lit_state: u32,
        lc: u32,
        range_decoder: &mut RangeDecoder<'_>,
    ) -> Result<u8, LzmaError> {
        let base_offset = 3 * (lit_state << lc);
        let mut symbol = 1u32;
        while symbol < SYMBOL_DONE {
            let idx = (base_offset + symbol) as usize;
            let bit = range_decoder.decode_bit(&mut self.models[idx])?;
            symbol = (symbol << 1) | bit;
        }
        // symbol is now in 0x100..=0x1FF; subtract 0x100 for the byte.
        Ok((symbol - SYMBOL_DONE) as u8)
    }

    /// Decode a byte in matched mode — the previous packet was a match,
    /// so the byte at the last match distance provides additional context.
    ///
    /// Implements the SDK's `rc_matched_literal` algorithm: process bits
    /// in pairs (match bit, decoded bit), switching to unmatched mode
    /// once they diverge.
    ///
    /// # Errors
    ///
    /// Forwards any [`LzmaError`] from the range decoder.
    pub fn decode_matched(
        &mut self,
        match_byte: u8,
        lit_state: u32,
        lc: u32,
        range_decoder: &mut RangeDecoder<'_>,
    ) -> Result<u8, LzmaError> {
        let base_offset = 3 * (lit_state << lc);
        let mut symbol = 1u32;
        let mut match_sym = u32::from(match_byte);
        let mut offset = SYMBOL_DONE; // 0x100

        loop {
            // Shift first, then extract — XZ Utils ordering.
            match_sym <<= 1;
            let match_bit = match_sym & offset;

            let model_idx = (base_offset + offset + match_bit + symbol) as usize;
            let bit = range_decoder.decode_bit(&mut self.models[model_idx])?;

            if bit == 0 {
                offset &= !match_bit;
                symbol <<= 1;
            } else {
                offset &= match_bit;
                symbol = (symbol << 1) | 1;
            }

            let match_bit_flag = u32::from(match_bit > 0);
            if match_bit_flag != bit {
                if symbol >= SYMBOL_DONE {
                    break;
                }
                return Self::decode_unmatched_tail(
                    symbol,
                    base_offset,
                    range_decoder,
                    &mut self.models,
                );
            }

            if symbol >= SYMBOL_DONE {
                break;
            }
        }

        Ok((symbol - SYMBOL_DONE) as u8)
    }

    /// Continue decoding in unmatched mode from a partial `symbol`.
    /// Extracted as an associated function so callers can pass a
    /// sub-slice of their model array.
    fn decode_unmatched_tail(
        mut symbol: u32,
        base_offset: u32,
        range_decoder: &mut RangeDecoder<'_>,
        models: &mut [BitModel],
    ) -> Result<u8, LzmaError> {
        while symbol < SYMBOL_DONE {
            let idx = (base_offset + symbol) as usize;
            let bit = range_decoder.decode_bit(&mut models[idx])?;
            symbol = (symbol << 1) | bit;
        }
        Ok((symbol - SYMBOL_DONE) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_with_given_capacity() {
        let d = LiteralDecoder::new(64);
        assert_eq!(d.models().len(), 64);
    }

    #[test]
    fn reset_restores_all_models() {
        let mut d = LiteralDecoder::new(16);
        // Pretend we adapted some models.
        for m in &mut d.models {
            m.update(0);
            m.update(0);
        }
        d.reset();
        for m in d.models {
            assert_eq!(m.probability(), crate::constants::INIT_PROBS);
        }
    }
}
