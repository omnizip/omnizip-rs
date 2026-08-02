//! Range encoder — the heart of LZMA compression.
//!
//! Ported line-by-line from
//! `omnizip/lib/omnizip/algorithms/lzma/range_encoder.rb` (202 LOC, MIT,
//! Ribose Inc.), which itself mirrors XZ Utils `range_encoder.h`.
//!
//! ## Range coding in one paragraph
//!
//! The encoder maintains an interval `[low, low + range)` inside a 32-bit
//! unsigned space (with `low` carrying into a high 32-bit overflow slot).
//! To encode a bit with probability `prob` of being 0, the interval is
//! split at `low + (range >> 11) * prob`. If the bit is 0, the new
//! range becomes that boundary; if 1, the boundary becomes the new
//! `low` and `range` shrinks by the same amount. When `range < TOP`
//! (`< 2^24`), a byte is emitted and the interval is renormalised.
//!
//! ## Determinism
//!
//! All arithmetic uses 32-bit/64-bit unsigned integers with explicit
//! masking to match XZ Utils' 32-bit semantics. Output is appended
//! sequentially to a `Vec<u8>` (no hash maps, no thread state).

#![forbid(unsafe_code)]

use crate::bit_model::BitModel;
use crate::constants::TOP;

/// Initial range value (matches XZ Utils `rc_reset()`).
const INITIAL_RANGE: u32 = 0xFFFF_FFFF;

/// Range encoder state. Writes encoded bytes to an internal `Vec<u8>`
/// and exposes them via [`Self::finish`].
#[derive(Debug)]
pub struct RangeEncoder {
    out: Vec<u8>,
    low: u64,
    range: u32,
    cache: u8,
    /// `cache_size` counts pending 0xFF bytes waiting for carry
    /// propagation. XZ Utils initialises to 1 (not 0).
    cache_size: u64,
    /// Position in `out` before the 5-byte flush padding. For LZMA2,
    /// the decoder only consumes bytes up to this point.
    pre_flush_pos: Option<usize>,
}

impl Default for RangeEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl RangeEncoder {
    /// Construct a fresh encoder with default state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            low: 0,
            range: INITIAL_RANGE,
            cache: 0,
            cache_size: 1,
            pre_flush_pos: None,
        }
    }

    /// Encode a single bit using `model`, updating the model in place.
    ///
    /// Matches XZ Utils `rc_encode_bit` — normalisation happens BEFORE
    /// the bit encoding.
    pub fn encode_bit(&mut self, model: &mut BitModel, bit: u32) {
        self.normalize();
        let prob = u32::from(model.probability());
        if bit == 0 {
            // RC_BIT_0: shrink range to lower portion.
            self.range = (self.range >> 11) * prob;
        } else {
            // RC_BIT_1: shift low up, shrink range by bound.
            let bound = prob * (self.range >> 11);
            self.low = self.low.wrapping_add(u64::from(bound));
            self.range -= bound;
        }
        model.update(bit);
    }

    /// Encode `num_bits` bits of `value` directly (uniform distribution).
    pub fn encode_direct_bits(&mut self, value: u32, num_bits: u32) {
        for i in (1..=num_bits).rev() {
            self.normalize();
            self.range >>= 1;
            let bit = (value >> (i - 1)) & 1;
            if bit == 1 {
                self.low = self.low.wrapping_add(u64::from(self.range));
            }
        }
    }

    /// Encode a symbol via cumulative frequency range (PPMd-style).
    pub fn encode_freq(&mut self, cum_freq: u32, freq: u32, total_freq: u32) {
        self.normalize();
        let range_freq = self.range / total_freq;
        let low_bound = range_freq * cum_freq;
        let high_bound = range_freq * (cum_freq + freq);
        self.low = self.low.wrapping_add(u64::from(low_bound));
        self.range = high_bound - low_bound;
    }

    /// Flush remaining bytes to the output (5-byte padding).
    pub fn flush(&mut self) {
        self.pre_flush_pos = Some(self.out.len());
        // Prevent further normalisations.
        self.range = INITIAL_RANGE;
        for _ in 0..5 {
            self.shift_low();
        }
    }

    /// Number of bytes the decoder will consume (excludes the 5-byte
    /// flush padding). For LZMA2 compatibility.
    #[must_use]
    pub fn bytes_for_decode(&self) -> usize {
        self.pre_flush_pos.unwrap_or(self.out.len())
    }

    /// Take the encoded output, consuming the encoder.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        if self.pre_flush_pos.is_none() {
            self.flush();
        }
        self.out
    }

    /// Normalize the range when it becomes too small. Matches XZ
    /// Utils' `rc_normalize` — `shift_low` is called BEFORE `range` is
    /// shifted.
    pub fn normalize(&mut self) {
        while self.range < TOP {
            self.shift_low();
            self.range <<= 8;
        }
    }

    /// Half the range. Used by `encode_direct_bits`.
    pub fn range_div2(&mut self) {
        self.range >>= 1;
    }

    /// Add `range` to `low` (used by direct-bit encoding when bit == 0).
    pub fn add_range(&mut self) {
        self.low = self.low.wrapping_add(u64::from(self.range));
    }

    /// Shift the top byte of `low` to output, handling carry via the
    /// cache mechanism. Direct port of XZ Utils `rc_shift_low`.
    fn shift_low(&mut self) {
        let low_32 = self.low as u32;
        let carry = (self.low >> 32) as u8;

        if low_32 < 0xFF00_0000 || carry != 0 {
            loop {
                self.out.push(self.cache.wrapping_add(carry));
                self.cache = 0xFF;
                self.cache_size -= 1;
                if self.cache_size == 0 {
                    break;
                }
            }
            self.cache = ((low_32 >> 24) & 0xFF) as u8;
        }
        self.cache_size += 1;
        // Mask low to 24 bits then shift left 8 (drops the top byte
        // we've just cached).
        self.low = (u64::from(low_32 & 0x00FF_FFFF)) << 8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::range_coder::RangeDecoder;

    #[test]
    fn fresh_encoder_has_initial_state() {
        let e = RangeEncoder::new();
        assert_eq!(e.range, INITIAL_RANGE);
        assert_eq!(e.cache_size, 1);
        assert_eq!(e.low, 0);
    }

    #[test]
    fn encode_bit_zero_round_trips() {
        let mut enc = RangeEncoder::new();
        let mut model = BitModel::new();
        enc.encode_bit(&mut model, 0);
        enc.flush();
        let bytes = enc.finish();
        // Construct a decoder from the bytes (after the flush padding).
        // The decoder reads 5 init bytes from the start.
        let mut dec = RangeDecoder::new(&bytes).expect("init decoder");
        let mut model2 = BitModel::new();
        let bit = dec.decode_bit(&mut model2).expect("decode");
        assert_eq!(bit, 0);
    }

    #[test]
    fn encode_bit_one_round_trips() {
        let mut enc = RangeEncoder::new();
        let mut model = BitModel::new();
        enc.encode_bit(&mut model, 1);
        enc.flush();
        let bytes = enc.finish();
        let mut dec = RangeDecoder::new(&bytes).expect("init decoder");
        let mut model2 = BitModel::new();
        let bit = dec.decode_bit(&mut model2).expect("decode");
        assert_eq!(bit, 1);
    }

    #[test]
    fn determinism_same_bits_same_output() {
        let encode_once = || {
            let mut e = RangeEncoder::new();
            let mut m = BitModel::new();
            for i in 0..1000 {
                e.encode_bit(&mut m, (i * 7) & 1);
            }
            e.flush();
            e.finish()
        };
        let a = encode_once();
        let b = encode_once();
        assert_eq!(a, b, "encoder non-deterministic");
    }
}
