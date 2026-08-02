//! PPM model using the proven ZPAQ arithmetic coder design.
//!
//! Each byte is encoded as 8 bits (MSB first). The prediction context
//! for bit `k` of byte `n` combines the byte-level context (last
//! `order` bytes hashed to u16) with the bit position.

#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_lossless)]

const PRECISION: u32 = 32;
const HALF: u64 = 1u64 << (PRECISION - 1);
const QUARTER: u64 = 1u64 << (PRECISION - 2);
const THREE_Q: u64 = 3 * QUARTER;
const MASK: u32 = u32::MAX;
const PROB_SCALE: u64 = 65536;

// ── Arithmetic encoder ──────────────────────────────────────────────

pub struct ArithEncoder {
    low: u64,
    high: u64,
    pending_ff: u64,
    out: Vec<u8>,
    bit_buf: u32,
    bit_count: u32,
}

impl ArithEncoder {
    pub fn new(_out: &mut Vec<u8>) -> Self {
        Self { low: 0, high: u64::from(MASK), pending_ff: 0, out: Vec::new(), bit_buf: 0, bit_count: 0 }
    }

    pub fn encode_bit(&mut self, prob: u16, bit: bool) {
        let range = self.high - self.low + 1;
        let split = self.low + range - (range * u64::from(prob)) / PROB_SCALE;
        if bit { self.low = split; } else { self.high = split - 1; }
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
            } else { break; }
            self.low <<= 1;
            self.high = (self.high << 1) | 1;
        }
    }

    fn emit_bit(&mut self, bit: bool) {
        self.push_bit(bit);
        for _ in 0..self.pending_ff { self.push_bit(!bit); }
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

    pub fn flush(mut self, out: &mut Vec<u8>) {
        self.pending_ff += 1;
        if self.low >= QUARTER { self.emit_bit(true); } else { self.emit_bit(false); }
        while self.bit_count != 0 { self.push_bit(false); }
        out.extend_from_slice(&self.out);
    }
}

// ── Arithmetic decoder ──────────────────────────────────────────────

pub struct ArithDecoder<'a> {
    low: u64, high: u64, code: u64,
    data: &'a [u8], pos: usize,
    bit_buf: u32, bit_count: u32,
}

impl<'a> ArithDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let mut s = Self { low: 0, high: u64::from(MASK), code: 0, data, pos: 0, bit_buf: 0, bit_count: 0 };
        for _ in 0..PRECISION { let b = s.read_bit(); s.code = (s.code << 1) | u64::from(b); }
        s
    }

    fn read_bit(&mut self) -> u8 {
        if self.bit_count == 0 {
            self.bit_buf = u32::from(if self.pos < self.data.len() { let b = self.data[self.pos]; self.pos += 1; b } else { 0 });
            self.bit_count = 8;
        }
        self.bit_count -= 1;
        ((self.bit_buf >> self.bit_count) & 1) as u8
    }

    pub fn decode_bit(&mut self, prob: u16) -> bool {
        let range = self.high - self.low + 1;
        let split = self.low + range - (range * u64::from(prob)) / PROB_SCALE;
        let bit = self.code > split - 1;
        if bit { self.low = split; } else { self.high = split - 1; }
        loop {
            if self.high < HALF {
            } else if self.low >= HALF {
                self.low -= HALF; self.high -= HALF; self.code -= HALF;
            } else if self.low >= QUARTER && self.high < THREE_Q {
                self.low -= QUARTER; self.high -= QUARTER; self.code -= QUARTER;
            } else { break; }
            self.low <<= 1;
            self.high = (self.high << 1) | 1;
            self.code = (self.code << 1) | u64::from(self.read_bit());
        }
        bit
    }
}

// ── PPM model ───────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct BitModel { n0: u16, n1: u16 }

impl BitModel {
    const fn new() -> Self { Self { n0: 1, n1: 1 } }
    fn prob1(&self) -> u16 {
        let t = u32::from(self.n0) + u32::from(self.n1);
        (((u32::from(self.n1) << 16) + t / 2) / t).clamp(1, 65535) as u16
    }
    fn update(&mut self, bit: bool) {
        if bit { self.n1 = self.n1.saturating_add(1); } else { self.n0 = self.n0.saturating_add(1); }
        if self.n0 + self.n1 > 1 << 12 { self.n0 = (self.n0 + 1) >> 1; self.n1 = (self.n1 + 1) >> 1; }
    }
}

pub struct PpmModel {
    history: Vec<u8>,
    order: usize,
    /// 1M-slot probability table (4 MB fixed memory). Each slot holds
    /// a per-bit adaptive model. The table is indexed by a u32 hash
    /// of the byte context combined with the bit position.
    models: Vec<BitModel>,
}

/// Table size: 2^20 = 1M entries. Uses 4 MB of memory (1M × 4 bytes).
/// This provides far better distribution than the old 64K table which
/// caused 681% expansion on large inputs due to hash collisions.
const TABLE_SIZE: usize = 1 << 20;
const TABLE_MASK: usize = TABLE_SIZE - 1;

impl PpmModel {
    pub fn new(order: usize) -> Self {
        Self { history: Vec::new(), order, models: vec![BitModel::new(); TABLE_SIZE] }
    }

    /// Hash the last `order` bytes to a u32 context key.
    fn ctx_hash(&self) -> u32 {
        let len = self.history.len().min(self.order);
        if len == 0 { return 0; }
        let start = self.history.len() - len;
        let mut h: u32 = 5381;
        for &b in &self.history[start..] { h = h.wrapping_mul(33).wrapping_add(u32::from(b)); }
        h
    }

    pub fn encode_byte(&mut self, enc: &mut ArithEncoder, byte: u8) {
        let ctx = self.ctx_hash();
        for bp in (0..8u32).rev() {
            let bit = ((byte >> bp) & 1) == 1;
            let idx = ((ctx.wrapping_mul(8).wrapping_add(bp)) as usize) & TABLE_MASK;
            let prob = self.models[idx].prob1();
            enc.encode_bit(prob, bit);
            self.models[idx].update(bit);
        }
        self.history.push(byte);
    }

    pub fn decode_byte(&mut self, dec: &mut ArithDecoder) -> u8 {
        let ctx = self.ctx_hash();
        let mut byte = 0u8;
        for bp in (0..8u32).rev() {
            let idx = ((ctx.wrapping_mul(8).wrapping_add(bp)) as usize) & TABLE_MASK;
            let prob = self.models[idx].prob1();
            let bit = dec.decode_bit(prob);
            if bit { byte |= 1 << bp; }
            self.models[idx].update(bit);
        }
        self.history.push(byte);
        byte
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_single_byte() {
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        { let mut enc = ArithEncoder::new(&mut buf); m.encode_byte(&mut enc, b'A'); enc.flush(&mut buf); }
        let mut m2 = PpmModel::new(4);
        let mut dec = ArithDecoder::new(&buf);
        assert_eq!(m2.decode_byte(&mut dec), b'A');
    }

    #[test]
    fn round_trip_text() {
        let text = b"The quick brown fox jumps over the lazy dog. ".repeat(10);
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        { let mut enc = ArithEncoder::new(&mut buf); for &b in &text { m.encode_byte(&mut enc, b); } enc.flush(&mut buf); }
        let mut m2 = PpmModel::new(4);
        let mut dec = ArithDecoder::new(&buf);
        let out: Vec<u8> = (0..text.len()).map(|_| m2.decode_byte(&mut dec)).collect();
        assert_eq!(out, text);
    }

    #[test]
    fn round_trip_all_bytes() {
        let data: Vec<u8> = (0..=255u16).map(|i| i as u8).collect();
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        { let mut enc = ArithEncoder::new(&mut buf); for &b in &data { m.encode_byte(&mut enc, b); } enc.flush(&mut buf); }
        let mut m2 = PpmModel::new(4);
        let mut dec = ArithDecoder::new(&buf);
        let out: Vec<u8> = (0..data.len()).map(|_| m2.decode_byte(&mut dec)).collect();
        assert_eq!(out, data);
    }

    #[test]
    fn compresses_repetitive() {
        let text = b"hello world ".repeat(100);
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        { let mut enc = ArithEncoder::new(&mut buf); for &b in &text { m.encode_byte(&mut enc, b); } enc.flush(&mut buf); }
        let ratio = buf.len() as f64 / text.len() as f64;
        eprintln!("ratio: {ratio:.3}");
        assert!(ratio < 0.50, "ratio {ratio:.3} >= 0.50");
    }

    #[test]
    fn determinism() {
        let mk = || { let mut m = PpmModel::new(4); let mut b = Vec::new(); let mut e = ArithEncoder::new(&mut b); for &c in b"test" { m.encode_byte(&mut e, c); } e.flush(&mut b); b };
        assert_eq!(mk(), mk());
    }
}
