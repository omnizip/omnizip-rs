//! Binary arithmetic coder.
//!
//! Classic 32-bit range-coding style arithmetic coder using the "E1/E2/E3
//! scaling" formulation:
//!
//! - The current sub-range is `[low, high]` inclusive, both `u32`.
//! - After narrowing, we check three renormalisation cases:
//!   - **E1** (output of `0` bit pending): `high < HALF`. Emit `0`, followed
//!     by `pending_ff` `1` bits, then shift.
//!   - **E2** (output of `1` bit pending): `low >= HALF`. Subtract `HALF`,
//!     emit `1` followed by `pending_ff` `0` bits, then shift.
//!   - **E3** (range straddles `HALF`): `low >= QUARTER && high < THREE_Q`.
//!     Decrement `pending_ff` (a carry-deferred marker); subtract `QUARTER`
//!     from both bounds; shift.
//!
//! After all bits are encoded, `finish()` flushes by emitting two more
//! renormalisation rounds to disambiguate the final sub-range.
//!
//! The probability is expressed as a `u16` in `[1, 65535]` representing
//! `P(bit=1) = prob / 65536`. Bits are emitted MSB-first inside each byte.

#![forbid(unsafe_code)]

// ---------------------------------------------------------------------------
// Constants — using a 32-bit precision with byte-wise output.
// ---------------------------------------------------------------------------

/// Total bits of precision in the coder's registers.
const PRECISION: u32 = 32;

const HALF: u64 = 1 << (PRECISION - 1); // midpoint
const QUARTER: u64 = 1 << (PRECISION - 2); // 0x4000_0000
const THREE_Q: u64 = 3 * QUARTER; // 0xC000_0000
const MASK: u32 = u32::MAX; // 0xFFFF_FFFF

/// Probability scale. The arithmetic operates on probabilities in this
/// 16-bit space; the range computation uses `u64` to avoid overflow.
pub const PROB_SCALE: u64 = 65536;

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Binary arithmetic encoder.
#[derive(Debug)]
pub struct ArithmeticEncoder {
    low: u64,
    high: u64,
    /// Number of pending E3-scaling bits waiting to be resolved.
    pending_ff: u64,
    out: Vec<u8>,
    /// Bit accumulator for assembling emitted bits into bytes.
    bit_buf: u32,
    bit_count: u32,
}

impl Default for ArithmeticEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ArithmeticEncoder {
    /// Construct a new encoder with an empty output buffer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            low: 0,
            high: u64::from(MASK),
            pending_ff: 0,
            out: Vec::new(),
            bit_buf: 0,
            bit_count: 0,
        }
    }

    /// Encode a single bit given `prob = P(bit=1) * 65536`.
    ///
    /// `prob` must be in `[1, 65535]`.
    pub fn encode_bit(&mut self, prob: u16, bit: bool) {
        debug_assert!((1..=u16::MAX).contains(&prob));

        let range = self.high - self.low + 1;
        // The boundary `split` partitions the range so that the upper part
        // (bit=1) has measure `prob / 65536 * range` and the lower part
        // (bit=0) has the remaining measure. Equivalently, bit=0 occupies
        // `[low, split)` and bit=1 occupies `[split, high]`.
        let split = self.low + range - (range * u64::from(prob)) / PROB_SCALE;

        if bit {
            self.low = split;
        } else {
            self.high = split - 1;
        }

        // Renormalise.
        loop {
            if self.high < HALF {
                // E1: emit 0 with `pending_ff` 1s after it.
                self.emit_bit(false);
            } else if self.low >= HALF {
                // E2: emit 1 with `pending_ff` 0s after it.
                self.emit_bit(true);
                self.low -= HALF;
                self.high -= HALF;
            } else if self.low >= QUARTER && self.high < THREE_Q {
                // E3: defer — increment pending counter, slide by QUARTER.
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

    /// Push a single bit (plus pending E3 bits) into the bit buffer.
    fn emit_bit(&mut self, bit: bool) {
        // First the actual bit.
        self.push_bit(bit);
        // Then `pending_ff` inverted copies.
        for _ in 0..self.pending_ff {
            self.push_bit(!bit);
        }
        self.pending_ff = 0;
    }

    fn push_bit(&mut self, bit: bool) {
        self.bit_buf = (self.bit_buf << 1) | u32::from(bit);
        self.bit_count += 1;
        if self.bit_count == 8 {
            self.out
                .push(u8::try_from(self.bit_buf).expect("8-bit buffer"));
            self.bit_buf = 0;
            self.bit_count = 0;
        }
    }

    /// Finalise the encoder, flushing remaining bits.
    ///
    /// To uniquely identify the final sub-range we must emit at least
    /// `ceil(log2(range_bits))` more bits. We emit one bit per
    /// `PRECISION - 2` shift to pin the range down. The standard trick:
    /// bump `pending_ff` by 1 (marking that one E3 carry is pending), then
    /// emit a bit chosen to lie inside the current range — `1` if
    /// `low >= QUARTER` (forcing the upper half), `0` otherwise.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        // Resolve the final sub-range by emitting the top bit of `low`
        // (after one E3 shift). Because `low >= QUARTER && high < THREE_Q`
        // is the only remaining case after the renormalise loop, picking
        // the QUARTER boundary selects which half we want.
        self.pending_ff += 1;
        if self.low >= QUARTER {
            self.emit_bit(true);
        } else {
            self.emit_bit(false);
        }
        // Pad any partial byte with zeros so the decoder can read 4 bytes.
        while self.bit_count != 0 {
            self.push_bit(false);
        }
        self.out
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Binary arithmetic decoder.
#[derive(Debug)]
pub struct ArithmeticDecoder<'a> {
    low: u64,
    high: u64,
    code: u64,
    data: &'a [u8],
    pos: usize,
    /// Bit accumulator mirroring the encoder.
    bit_buf: u32,
    bit_count: u32,
}

impl<'a> ArithmeticDecoder<'a> {
    /// Construct a decoder seeded from `data`. The first `PRECISION` bits
    /// (32) seed `code`.
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
            self.bit_buf = u32::from(next_byte(self.data, &mut self.pos));
            self.bit_count = 8;
        }
        self.bit_count -= 1;
        ((self.bit_buf >> self.bit_count) & 1) as u8
    }

    /// Decode a single bit given `prob = P(bit=1) * 65536`.
    pub fn decode_bit(&mut self, prob: u16) -> bool {
        debug_assert!((1..=u16::MAX).contains(&prob));

        let range = self.high - self.low + 1;
        let split = self.low + range - (range * u64::from(prob)) / PROB_SCALE;

        let bit = self.code > split - 1;
        if bit {
            self.low = split;
        } else {
            self.high = split - 1;
        }

        // Renormalise — must mirror the encoder exactly.
        loop {
            if self.high < HALF {
                // E1 — no adjustment.
            } else if self.low >= HALF {
                // E2.
                self.low -= HALF;
                self.high -= HALF;
                self.code -= HALF;
            } else if self.low >= QUARTER && self.high < THREE_Q {
                // E3.
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

fn next_byte(data: &[u8], pos: &mut usize) -> u8 {
    if *pos < data.len() {
        let b = data[*pos];
        *pos += 1;
        b
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
mod tests {
    use super::*;

    fn round_trip_bits(bits: &[bool], probs: &[u16], label: &str) {
        assert_eq!(bits.len(), probs.len());
        let mut enc = ArithmeticEncoder::new();
        for (i, &b) in bits.iter().enumerate() {
            enc.encode_bit(probs[i], b);
        }
        let bytes = enc.finish();
        let mut dec = ArithmeticDecoder::new(&bytes);
        for (i, expected) in bits.iter().enumerate() {
            let got = dec.decode_bit(probs[i]);
            assert_eq!(got, *expected, "{label}: bit {i} mismatch");
        }
    }

    #[test]
    fn round_trip_all_zeros_uniform() {
        let bits = vec![false; 2048];
        let probs = vec![1 << 15; 2048];
        round_trip_bits(&bits, &probs, "all-zeros-uniform");
    }

    #[test]
    fn round_trip_all_ones_uniform() {
        let bits = vec![true; 2048];
        let probs = vec![1 << 15; 2048];
        round_trip_bits(&bits, &probs, "all-ones-uniform");
    }

    #[test]
    fn round_trip_alternating() {
        let n = 4096;
        let bits: Vec<bool> = (0..n).map(|i| i % 2 == 1).collect();
        let probs = vec![1 << 15; n];
        round_trip_bits(&bits, &probs, "alternating");
    }

    #[test]
    fn round_trip_pseudo_random_uniform() {
        let mut x: u64 = 0x1234_5678;
        let bits: Vec<bool> = (0..4096)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x & 1) == 1
            })
            .collect();
        let probs = vec![1 << 15; 4096];
        round_trip_bits(&bits, &probs, "xorshift-uniform");
    }

    #[test]
    fn round_trip_long_run_then_flip() {
        let mut bits = vec![false; 8192];
        bits.push(true);
        bits.push(false);
        bits.extend_from_slice(&vec![true; 4096]);
        bits.push(false);
        let probs = vec![1 << 15; bits.len()];
        round_trip_bits(&bits, &probs, "long-run-then-flip");
    }

    /// Random probabilities — stresses carry propagation thoroughly.
    #[test]
    fn round_trip_random_probabilities() {
        let mut x: u64 = 0xDEAD_BEEF_CAFE_BABE;
        let mut rng = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for trial in 0..30 {
            let n_bits = 200 + (rng() % 1000) as usize;
            let bits: Vec<bool> = (0..n_bits).map(|_| rng() & 1 == 1).collect();
            let probs: Vec<u16> = (0..n_bits)
                .map(|_| 1 + (rng() % (u64::from(u16::MAX) - 1)) as u16)
                .collect();
            round_trip_bits(&bits, &probs, &format!("random-probs trial {trial}"));
        }
    }

    /// Extreme probabilities (1 and 65535) — degenerate sub-ranges.
    #[test]
    fn round_trip_extreme_probs() {
        let bits: Vec<bool> = (0..1024).map(|i| i % 7 == 0).collect();
        let probs: Vec<u16> = bits.iter().map(|&b| if b { 65535 } else { 1 }).collect();
        round_trip_bits(&bits, &probs, "extreme-probs");
    }
}
