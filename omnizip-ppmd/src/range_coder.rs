//! Range coder — clean Subbotin implementation with correct carry
//! propagation.
//!
//! Uses a u64 `low` register (33 bits effective) and a deferred-byte
//! cache to handle carries. The encoder and decoder stay in lockstep
//! as long as the model produces identical `(total, sym_lo, sym_hi)`
//! triples on both sides.

#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_lossless)]

const TOP: u64 = 1u64 << 24;
const BOT: u64 = 1u64 << 16;

// ── Encoder ─────────────────────────────────────────────────────────

pub struct RangeEncoder<'a> {
    out: &'a mut Vec<u8>,
    low: u64,
    range: u32,
    cache: u8,
    ff_count: u64,
}

impl<'a> RangeEncoder<'a> {
    pub fn new(out: &'a mut Vec<u8>) -> Self {
        Self { out, low: 0, range: 0xFFFF_FFFF, cache: 0, ff_count: 0 }
    }

    /// Encode symbol `[sym_lo, sym_hi)` within `[0, total)`.
    pub fn encode(&mut self, sym_lo: u32, sym_hi: u32, total: u32) {
        debug_assert!(total >= 1 && sym_hi > sym_lo && sym_hi <= total);
        let r = self.range / total;
        self.low += u64::from(sym_lo) * u64::from(r);
        self.range = (sym_hi - sym_lo) * r;
        self.renorm();
    }

    fn renorm(&mut self) {
        while (self.low ^ (self.low + u64::from(self.range))) < TOP {
            self.shift_out();
        }
        while u64::from(self.range) < BOT {
            self.shift_out();
        }
    }

    /// Emit the top byte of `low`, handling carries via the cache.
    fn shift_out(&mut self) {
        let top_byte = (self.low >> 24) as u8;
        let carry = self.low >= 0x1_0000_0000;

        if carry || top_byte != 0xFF {
            // Top byte is settled — emit cache (with carry) and pending 0xFFs.
            let byte = self.cache.wrapping_add(if carry { 1 } else { 0 });
            self.out.push(byte);
            let fill = if carry { 0x00 } else { 0xFF };
            for _ in 0..self.ff_count {
                self.out.push(fill);
            }
            self.ff_count = 0;
            self.cache = top_byte;
        } else {
            // Top byte is 0xFF with no carry yet — defer.
            self.ff_count += 1;
        }

        self.low = (self.low << 8) & 0xFFFF_FFFF;
    }

    pub fn flush(&mut self) {
        for _ in 0..5 {
            self.shift_out();
        }
    }
}

// ── Decoder ─────────────────────────────────────────────────────────

pub struct RangeDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    code: u32,
    range: u32,
}

impl<'a> RangeDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let mut s = Self { data, pos: 0, code: 0, range: 0xFFFF_FFFF };
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

    /// The cumulative-frequency target within `[0, total)`.
    #[must_use]
    pub fn target_freq(&self, total: u32) -> u32 {
        let r = self.range / total;
        self.code / r
    }

    /// Advance past symbol `[sym_lo, sym_hi)` in `total`.
    pub fn advance(&mut self, sym_lo: u32, sym_hi: u32, total: u32) {
        debug_assert!(total >= 1 && sym_hi > sym_lo && sym_hi <= total);
        let r = (self.range / total).max(1);
        self.code = self.code.wrapping_sub(sym_lo.wrapping_mul(r));
        self.range = (sym_hi - sym_lo) * r;
        self.renorm();
    }

    fn renorm(&mut self) {
        loop {
            let combined = self.code.wrapping_add(self.range);
            let top_ok = (u64::from(self.code) ^ u64::from(combined)) >= TOP;
            let range_ok = u64::from(self.range) >= BOT;
            if top_ok && range_ok {
                break;
            }
            self.shift_in();
            // Guard against infinite loop if range somehow reaches 0.
            if self.range == 0 {
                self.range = 0xFFFF_FFFF;
                break;
            }
        }
    }

    fn shift_in(&mut self) {
        self.code = (self.code << 8) | u32::from(self.read_byte());
        self.range <<= 8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_uniform() {
        let n = 500u32;
        let mut buf = Vec::new();
        {
            let mut enc = RangeEncoder::new(&mut buf);
            for i in 0..n {
                let bit = i % 2;
                if bit == 0 {
                    enc.encode(0, 1, 2);
                } else {
                    enc.encode(1, 2, 2);
                }
            }
            enc.flush();
        }
        let mut dec = RangeDecoder::new(&buf);
        for i in 0..n {
            let t = dec.target_freq(2);
            assert!(t < 2);
            let bit = if t < 1 { 0u32 } else { 1 };
            if bit == 0 {
                dec.advance(0, 1, 2);
            } else {
                dec.advance(1, 2, 2);
            }
            assert_eq!(bit, i % 2, "mismatch at {i}");
        }
    }

    #[test]
    fn round_trip_biased() {
        let n = 2000u32;
        let mut buf = Vec::new();
        {
            let mut enc = RangeEncoder::new(&mut buf);
            for i in 0..n {
                if i % 5 == 0 {
                    enc.encode(7, 8, 8);
                } else {
                    enc.encode(0, 7, 8);
                }
            }
            enc.flush();
        }
        let mut dec = RangeDecoder::new(&buf);
        for i in 0..n {
            let t = dec.target_freq(8);
            let sym = if t < 7 { 0u32 } else { 1 };
            if sym == 0 {
                dec.advance(0, 7, 8);
            } else {
                dec.advance(7, 8, 8);
            }
            assert_eq!(sym, if i % 5 == 0 { 1 } else { 0 }, "mismatch at {i}");
        }
    }

    #[test]
    fn round_trip_multi_symbol() {
        let symbols: Vec<u32> = (0..1000).map(|i| (i * 37) % 256).collect();
        let mut buf = Vec::new();
        {
            let mut enc = RangeEncoder::new(&mut buf);
            for &s in &symbols {
                enc.encode(s, s + 1, 256);
            }
            enc.flush();
        }
        let mut dec = RangeDecoder::new(&buf);
        for (i, &expected) in symbols.iter().enumerate() {
            let t = dec.target_freq(256);
            let s = t.min(255);
            dec.advance(s, s + 1, 256);
            assert_eq!(s, expected, "symbol mismatch at {i}");
        }
    }
}
