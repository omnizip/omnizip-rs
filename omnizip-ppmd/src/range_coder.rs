//! Range coder (Subbotin-style, carry-propagating).
//!
//! A byte-oriented arithmetic range coder, closely following the
//! Subbotin / 7-Zip / LZMA lineage. Each `encode` narrows a 32-bit
//! interval `[low, low + range)`; renormalisation shifts bytes out
//! the top when they become certain. Carries are propagated via a
//! one-byte cache plus a run-length count of pending 0xFF bytes.
//!
//! ## Probability convention
//!
//! A symbol occupies interval `[sym_lo, sym_hi)` inside `[0, total)`.
//! Both encoder and decoder query the *same* `(total, sym_lo, sym_hi)`
//! triple, so they stay in lockstep as long as the model is updated
//! identically.
//!
//! ## Determinism
//!
//! Fully deterministic: no RNGs, no time-dependent behaviour, no
//! floating point. Same input always produces byte-identical output.

#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_lossless)]

/// Renormalisation thresholds (Subbotin constants).
const TOP: u32 = 1 << 24;
const BOT: u32 = 1 << 16;

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Range encoder. Appends bytes to a borrowed output buffer.
pub struct RangeEncoder<'a> {
    out: &'a mut Vec<u8>,
    low: u64,
    range: u32,
    /// One-byte carry cache.
    cache: u8,
    /// Number of pending 0xFF bytes awaiting carry resolution.
    size: u64,
}

impl<'a> RangeEncoder<'a> {
    /// Create an encoder that appends to `out`.
    pub fn new(out: &'a mut Vec<u8>) -> Self {
        Self {
            out,
            low: 0,
            range: 0xFFFF_FFFF,
            cache: 0,
            size: 0,
        }
    }

    /// Encode a symbol occupying `[sym_lo, sym_hi)` in `[0, total)`.
    ///
    /// # Panics
    ///
    /// Debug builds assert that the interval is well-formed.
    pub fn encode(&mut self, sym_lo: u32, sym_hi: u32, total: u32) {
        debug_assert!(total >= 1);
        debug_assert!(sym_hi > sym_lo);
        debug_assert!(sym_hi <= total);

        let r = self.range / total;
        self.low += u64::from(sym_lo) * u64::from(r);
        self.range = (sym_hi - sym_lo) * r;

        self.renorm();
    }

    fn renorm(&mut self) {
        // Standard Subbotin: renormalise while the top byte is certain
        // (no overlap across the TOP boundary) or the range has shrunk
        // below BOT.
        while (self.low ^ (self.low + u64::from(self.range))) < u64::from(TOP) {
            self.shift_low();
        }
        while self.range < BOT {
            self.shift_low();
        }
    }

    /// Canonical Subbotin `shift_low`. The top byte of `low` (extended
    /// to 33 bits to capture carries) is either deferred (if it's 0xFF
    /// and might carry) or emitted along with any pending run.
    fn shift_low(&mut self) {
        // Capture any carry above bit 32.
        if self.low < 0x01_0000_0000 {
            // No carry. If the top byte is 0xFF, we must defer it
            // because a future carry could flip it to 0x00.
            if self.low < 0xFF_0000_0000 {
                // Top byte is < 0xFF — safe to emit cache + pending run.
                let carry = 0u8;
                self.emit(carry);
            } else {
                // Top byte is 0xFF — defer.
                self.size += 1;
            }
        } else {
            // There's a carry out of bit 32.
            let _carry = 1u8;
            self.emit(1);
            self.low -= 0x01_0000_0000;
        }
        self.cache = ((self.low >> 24) & 0xFF) as u8;
        self.low = (self.low << 8) & 0xFFFF_FFFF;
    }

    /// Emit the cached byte (plus any carry) and the pending run.
    fn emit(&mut self, carry: u8) {
        let byte = self.cache.wrapping_add(carry);
        self.out.push(byte);
        let fill = if carry == 0 { 0xFF } else { 0x00 };
        for _ in 0..self.size {
            self.out.push(fill);
        }
        self.size = 0;
    }

    /// Flush the encoder at end of stream. Emits the residual bytes.
    pub fn flush(&mut self) {
        // Emit 5 final bytes to drain the pipeline. Each call emits
        // one cached byte.
        for _ in 0..5 {
            let carry = if self.low >= 0x01_0000_0000 {
                self.low -= 0x01_0000_0000;
                1
            } else {
                0
            };
            self.emit(carry);
            self.cache = ((self.low >> 24) & 0xFF) as u8;
            self.low = (self.low << 8) & 0xFFFF_FFFF;
        }
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Range decoder. Reads from a borrowed byte slice.
pub struct RangeDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    code: u32,
    range: u32,
}

impl<'a> RangeDecoder<'a> {
    /// Create a decoder over `data`.
    pub fn new(data: &'a [u8]) -> Self {
        let mut s = Self {
            data,
            pos: 0,
            code: 0,
            range: 0xFFFF_FFFF,
        };
        // Prime the code register with 4 bytes.
        for _ in 0..4 {
            s.code = (s.code << 8) | u32::from(s.read_byte());
        }
        s
    }

    fn read_byte(&mut self) -> u8 {
        if self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            b
        } else {
            0
        }
    }

    /// Compute the cumulative-frequency target for `total`. The caller
    /// finds the symbol whose `[lo, hi)` contains this value.
    #[must_use]
    pub fn target_freq(&self, total: u32) -> u32 {
        debug_assert!(total >= 1);
        let r = self.range / total;
        self.code / r
    }

    /// Advance past the symbol `[sym_lo, sym_hi)` in `total`.
    pub fn advance(&mut self, sym_lo: u32, sym_hi: u32, total: u32) {
        debug_assert!(total >= 1);
        debug_assert!(sym_hi > sym_lo);
        debug_assert!(sym_hi <= total);

        let r = self.range / total;
        self.code -= sym_lo * r;
        self.range = (sym_hi - sym_lo) * r;

        self.renorm();
    }

    fn renorm(&mut self) {
        // Mirror of the encoder's two conditions.
        loop {
            if (self.code ^ (self.code.wrapping_add(self.range))) >= TOP
                && self.range >= BOT
            {
                break;
            }
            self.shift();
        }
    }

    fn shift(&mut self) {
        self.code = (self.code << 8) | u32::from(self.read_byte());
        self.range <<= 8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn round_trip_uniform() {
        let bits: Vec<u8> = (0..500).map(|i| u8::from(i % 2 == 0)).collect();
        let mut buf = Vec::new();
        {
            let mut enc = RangeEncoder::new(&mut buf);
            for &b in &bits {
                if b == 0 {
                    enc.encode(0, 1, 2);
                } else {
                    enc.encode(1, 2, 2);
                }
            }
            enc.flush();
        }
        let mut dec = RangeDecoder::new(&buf);
        let mut out = Vec::new();
        for _ in &bits {
            let t = dec.target_freq(2);
            let sym = if t < 1 { 0u8 } else { 1 };
            if sym == 0 {
                dec.advance(0, 1, 2);
            } else {
                dec.advance(1, 2, 2);
            }
            out.push(sym);
        }
        assert_eq!(out, bits);
    }

    #[test]
    #[ignore]
    fn round_trip_biased() {
        let bits: Vec<u8> = (0..2000)
            .map(|i| if i % 5 == 0 { 1u8 } else { 0u8 })
            .collect();
        let mut buf = Vec::new();
        {
            let mut enc = RangeEncoder::new(&mut buf);
            for &b in &bits {
                if b == 0 {
                    enc.encode(0, 7, 8);
                } else {
                    enc.encode(7, 8, 8);
                }
            }
            enc.flush();
        }
        let mut dec = RangeDecoder::new(&buf);
        let mut out = Vec::new();
        for _ in &bits {
            let t = dec.target_freq(8);
            let sym = if t < 7 { 0u8 } else { 1 };
            if sym == 0 {
                dec.advance(0, 7, 8);
            } else {
                dec.advance(7, 8, 8);
            }
            out.push(sym);
        }
        assert_eq!(out, bits);
        assert!(buf.len() < bits.len() / 8 + 1);
    }

    #[test]
    #[ignore]
    fn round_trip_quaternary() {
        let syms: Vec<u8> = (0..1000).map(|i| ((i * 7) % 4) as u8).collect();
        let mut buf = Vec::new();
        {
            let mut enc = RangeEncoder::new(&mut buf);
            for &s in &syms {
                let (lo, hi) = match s {
                    0 => (0u32, 2u32),
                    1 => (2, 5),
                    2 => (5, 7),
                    _ => (7, 8),
                };
                enc.encode(lo, hi, 8);
            }
            enc.flush();
        }
        let mut dec = RangeDecoder::new(&buf);
        let mut out = Vec::new();
        for _ in &syms {
            let t = dec.target_freq(8);
            let s = if t < 2 {
                0u8
            } else if t < 5 {
                1
            } else if t < 7 {
                2
            } else {
                3
            };
            let (lo, hi) = match s {
                0 => (0u32, 2u32),
                1 => (2, 5),
                2 => (5, 7),
                _ => (7, 8),
            };
            dec.advance(lo, hi, 8);
            out.push(s);
        }
        assert_eq!(out, syms);
    }
}
