//! Binary arithmetic coder (ZPAQ-style) shared across omnizip-rs
//! codecs.
//!
//! A 32-bit precision arithmetic coder with E1/E2/E3 carry handling
//! and a deferred-byte cache. The encoder and decoder stay in lockstep
//! as long as the model produces identical bit probabilities on both
//! sides.
//!
//! Used by PPMd7 and PPMd8. Kept here (in the shared codecs crate)
//! rather than duplicated in each PPMd module.
//!
//! ## Determinism
//!
//! The coder is a pure function of (input bits, probabilities). Same
//! inputs ⇒ byte-identical output across runs, machines, and Rust
//! versions.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

const PRECISION: u32 = 32;
const HALF: u64 = 1u64 << (PRECISION - 1);
const QUARTER: u64 = 1u64 << (PRECISION - 2);
const THREE_Q: u64 = 3 * QUARTER;
const MASK: u32 = u32::MAX;

/// Binary arithmetic encoder. Maintains a `[low, high)` interval
/// in `u64` and emits MSB-first.
pub struct ArithEncoder {
    low: u64,
    high: u64,
    pending_ff: u64,
    out: Vec<u8>,
    bit_buf: u32,
    bit_count: u32,
}

impl ArithEncoder {
    /// Construct. Output is buffered internally; `flush` writes it
    /// to the caller-supplied Vec.
    #[must_use]
    pub fn new() -> Self {
        Self {
            low: 0,
            high: u64::from(MASK),
            pending_ff: 0,
            out: Vec::with_capacity(4096),
            bit_buf: 0,
            bit_count: 0,
        }
    }

    /// Encode one bit given the probability of `bit == true`
    /// (in range `1..=65535`; `PROB_SCALE` itself is excluded).
    pub fn encode_bit(&mut self, prob: u16, bit: bool) {
        let range = self.high - self.low + 1;
        let split = self.low + range - (range * u64::from(prob)) / PROB_SCALE;
        if bit {
            self.low = split;
        } else {
            self.high = split - 1;
        }
        loop {
            if self.high < HALF {
                self.emit_bit(false);
            } else if self.low >= HALF {
                self.emit_bit(true);
                self.low -= HALF;
                self.high -= HALF;
            } else if self.low >= QUARTER && self.high < THREE_Q {
                self.pending_ff += 1;
                self.low -= QUARTER;
                self.high -= QUARTER;
            } else {
                break;
            }
            self.low <<= 1;
            self.high = (self.high << 1) | 1;
        }
    }

    fn emit_bit(&mut self, bit: bool) {
        self.push_bit(bit);
        for _ in 0..self.pending_ff {
            self.push_bit(!bit);
        }
        self.pending_ff = 0;
    }

    fn push_bit(&mut self, bit: bool) {
        self.bit_buf = (self.bit_buf << 1) | u32::from(bit);
        self.bit_count += 1;
        if self.bit_count == 8 {
            self.out.push(self.bit_buf as u8);
            self.bit_buf = 0;
            self.bit_count = 0;
        }
    }

    /// Finalise the stream and append encoded bytes to `out`.
    /// Consumes the encoder.
    pub fn flush(mut self, out: &mut Vec<u8>) {
        self.pending_ff += 1;
        if self.low >= QUARTER {
            self.emit_bit(true);
        } else {
            self.emit_bit(false);
        }
        while self.bit_count != 0 {
            self.push_bit(false);
        }
        out.extend_from_slice(&self.out);
    }
}

impl Default for ArithEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Binary arithmetic decoder. Mirrors [`ArithEncoder`].
pub struct ArithDecoder<'a> {
    low: u64,
    high: u64,
    code: u64,
    data: &'a [u8],
    pos: usize,
    bit_buf: u32,
    bit_count: u32,
}

impl<'a> ArithDecoder<'a> {
    /// Construct, priming the code register with the first
    /// `PRECISION` bits from `data`.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        let mut s = Self {
            low: 0,
            high: u64::from(MASK),
            code: 0,
            data,
            pos: 0,
            bit_buf: 0,
            bit_count: 0,
        };
        for _ in 0..PRECISION {
            let b = s.read_bit();
            s.code = (s.code << 1) | u64::from(b);
        }
        s
    }

    fn read_bit(&mut self) -> u8 {
        if self.bit_count == 0 {
            self.bit_buf = u32::from(if self.pos < self.data.len() {
                let b = self.data[self.pos];
                self.pos += 1;
                b
            } else {
                0
            });
            self.bit_count = 8;
        }
        self.bit_count -= 1;
        ((self.bit_buf >> self.bit_count) & 1) as u8
    }

    /// Decode one bit given the probability of `bit == true`.
    pub fn decode_bit(&mut self, prob: u16) -> bool {
        let range = self.high - self.low + 1;
        let split = self.low + range - (range * u64::from(prob)) / PROB_SCALE;
        let bit = self.code > split - 1;
        if bit {
            self.low = split;
        } else {
            self.high = split - 1;
        }
        loop {
            if self.high < HALF {
            } else if self.low >= HALF {
                self.low -= HALF;
                self.high -= HALF;
                self.code -= HALF;
            } else if self.low >= QUARTER && self.high < THREE_Q {
                self.low -= QUARTER;
                self.high -= QUARTER;
                self.code -= QUARTER;
            } else {
                break;
            }
            self.low <<= 1;
            self.high = (self.high << 1) | 1;
            self.code = (self.code << 1) | u64::from(self.read_bit());
        }
        bit
    }
}

/// Probability scale constant. Probabilities are `u16` in
/// `1..=65535`; `PROB_SCALE = 65536` represents certainty.
pub const PROB_SCALE: u64 = 65_536;

/// Scale a frequency to a probability in `[1, PROB_SCALE-1]`.
///
/// Returns `count / total` scaled to `PROB_SCALE`, with rounding.
/// Used by PPMd7 and PPMd8 to compute per-symbol probabilities for
/// the arithmetic coder. Extracted here to avoid duplication.
#[must_use]
pub fn scaled_prob(count: u32, total: u32) -> u16 {
    if total == 0 {
        return 1;
    }
    let p = (u64::from(count) * PROB_SCALE + u64::from(total) / 2) / u64::from(total);
    p.min(PROB_SCALE - 1).max(1) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode-decode a single bit at p=0.5, expect it back.
    #[test]
    fn round_trip_single_bit_uniform() {
        for bit in [false, true] {
            let mut buf = Vec::new();
            let mut enc = ArithEncoder::new();
            enc.encode_bit(32_768, bit);
            enc.flush(&mut buf);
            let mut dec = ArithDecoder::new(&buf);
            assert_eq!(dec.decode_bit(32_768), bit);
        }
    }

    /// Encode-decode a biased bit (p=0.9 for true).
    #[test]
    fn round_trip_single_bit_biased() {
        for bit in [false, true] {
            let mut buf = Vec::new();
            let mut enc = ArithEncoder::new();
            enc.encode_bit(58_982, bit); // ~0.9
            enc.flush(&mut buf);
            let mut dec = ArithDecoder::new(&buf);
            assert_eq!(dec.decode_bit(58_982), bit);
        }
    }

    /// Round-trip a byte stream with a constant probability per bit.
    #[test]
    fn round_trip_byte_stream_uniform() {
        let bytes = b"hello world";
        let mut buf = Vec::new();
        let mut enc = ArithEncoder::new();
        for &b in bytes {
            for bp in (0..8u32).rev() {
                enc.encode_bit(32_768, ((b >> bp) & 1) == 1);
            }
        }
        enc.flush(&mut buf);

        let mut dec = ArithDecoder::new(&buf);
        let mut out = Vec::new();
        for _ in bytes {
            let mut b: u8 = 0;
            for i in 0..8 {
                if dec.decode_bit(32_768) {
                    b |= 1 << (7 - i);
                }
            }
            out.push(b);
        }
        assert_eq!(out, bytes);
    }

    /// Determinism: same inputs → same output bytes.
    #[test]
    fn determinism() {
        let mk = || {
            let mut buf = Vec::new();
            let mut enc = ArithEncoder::new();
            for &c in b"deterministic test" {
                enc.encode_bit(32_768, c & 1 == 1);
            }
            enc.flush(&mut buf);
            buf
        };
        assert_eq!(mk(), mk());
    }
}
