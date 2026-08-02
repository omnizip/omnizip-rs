//! Order-2 context model with adaptive bit probabilities.
//!
//! The model maintains a probability estimate for the next bit, conditioned
//! on the last two bits observed in the current order-2 context (the two
//! most recently encoded bits). Each context has a pair of frequency
//! counters `(n0, n1)` updated after every coded bit; the probability
//! `P(bit=1)` returned to the arithmetic coder is
//! `(n1 + 1) / (n0 + n1 + 2)`, mapped to a `u16` in `[1, 65535]`.
//!
//! ## Why order-2 over the *bit* stream?
//!
//! A bit-level order-2 context adapts quickly and is cheap (only 16 entries
//! are needed). To get useful redundancy from a byte-level model we also fold
//! in the previous byte value as a second dimension: the context key is the
//! concatenation of the last two byte values (16 bits, 65 536 entries) plus
//! the current bit position within the byte. This is the canonical "order-2"
//! bit-context model used by simple PAQ-style coders.
//!
//! ## Storage
//!
//! A dense `[[Counter; 256]; 65536]` table would consume 32 MB. Instead we
//! use a `HashMap<(u16, u8), Counter>` so only contexts that actually occur
//! are stored — typically a few thousand entries for text inputs.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use crate::arithmetic::{ArithmeticDecoder, ArithmeticEncoder, PROB_SCALE};

/// Maximum count value before both counters are halved (preventing overflow
/// and providing gradual forgetting of old statistics).
const MAX_COUNT: u16 = 1 << 14;

/// A pair of bit-frequency counters.
#[derive(Clone, Copy, Debug, Default)]
struct Counter {
    n0: u16,
    n1: u16,
}

impl Counter {
    /// Probability that the next bit is 1, as a `u16` in `[1, 65535]`.
    fn prob_one(self) -> u16 {
        let denom = u64::from(self.n0) + u64::from(self.n1) + 2;
        let num = u64::from(self.n1) + 1;
        // Result fits in u64; map to [1, PROB_SCALE-1].
        let scaled = (num * (PROB_SCALE - 1)) / denom + 1;
        let cap = PROB_SCALE - 1;
        u16::try_from(scaled.min(cap)).unwrap_or(u16::MAX)
    }

    fn observe(&mut self, bit: bool) {
        if bit {
            self.n1 = self.n1.saturating_add(1);
        } else {
            self.n0 = self.n0.saturating_add(1);
        }
        if self.n0 >= MAX_COUNT || self.n1 >= MAX_COUNT {
            self.n0 = self.n0 / 2 + (self.n0 & 1);
            self.n1 = self.n1 / 2 + (self.n1 & 1);
        }
    }
}

/// Order-2 byte-context adaptive model. Stateless across encode/decode —
/// the model is rebuilt identically on both sides because it depends only
/// on already-decoded data.
pub struct Order2Model {
    table: HashMap<(u16, u8), Counter>,
    last_byte: u8,
    prev_byte: u8,
}

impl Default for Order2Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Order2Model {
    #[must_use]
    pub fn new() -> Self {
        Self {
            table: HashMap::new(),
            last_byte: 0,
            prev_byte: 0,
        }
    }

    /// Context key = (`prev_byte` || `last_byte`) packed as u16 + bit position.
    fn key(&self, bit_pos: u8) -> (u16, u8) {
        let ctx = (u16::from(self.prev_byte) << 8) | u16::from(self.last_byte);
        (ctx, bit_pos)
    }

    fn prob(&self, bit_pos: u8) -> u16 {
        match self.table.get(&self.key(bit_pos)) {
            Some(c) => c.prob_one(),
            None => 1 << 15,
        }
    }

    fn update(&mut self, bit_pos: u8, bit: bool) {
        let key = self.key(bit_pos);
        let entry = self.table.entry(key).or_default();
        entry.observe(bit);
    }

    fn advance_byte(&mut self, byte: u8) {
        self.prev_byte = self.last_byte;
        self.last_byte = byte;
    }

    /// Encode a byte MSB-first using the current model.
    pub fn encode_byte(&mut self, byte: u8, enc: &mut ArithmeticEncoder) {
        for bit_pos in 0..8 {
            let bit = (byte >> (7 - bit_pos)) & 1 == 1;
            let prob = self.prob(bit_pos);
            enc.encode_bit(prob, bit);
            self.update(bit_pos, bit);
        }
        self.advance_byte(byte);
    }

    /// Decode a byte MSB-first using the current model.
    pub fn decode_byte(&mut self, dec: &mut ArithmeticDecoder) -> u8 {
        let mut byte: u8 = 0;
        for bit_pos in 0..8 {
            let prob = self.prob(bit_pos);
            let bit = dec.decode_bit(prob);
            self.update(bit_pos, bit);
            if bit {
                byte |= 1 << (7 - bit_pos);
            }
        }
        self.advance_byte(byte);
        byte
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
mod tests {
    use super::*;

    fn round_trip(input: &[u8], label: &str) {
        let mut enc = ArithmeticEncoder::new();
        let mut model = Order2Model::new();
        for &b in input {
            model.encode_byte(b, &mut enc);
        }
        let bytes = enc.finish();

        let mut dec = ArithmeticDecoder::new(&bytes);
        let mut model = Order2Model::new();
        let mut out = Vec::with_capacity(input.len());
        for _ in 0..input.len() {
            out.push(model.decode_byte(&mut dec));
        }
        assert_eq!(out, input, "{label}: round-trip mismatch");
    }

    #[test]
    fn round_trip_empty() {
        round_trip(b"", "empty");
    }

    #[test]
    fn round_trip_single_byte() {
        round_trip(b"A", "single-byte");
    }

    #[test]
    fn round_trip_text() {
        let text = b"The quick brown fox jumps over the lazy dog. ".repeat(10);
        round_trip(&text, "text");
    }

    #[test]
    fn round_trip_binary_sequence() {
        let data: Vec<u8> = (0..=u8::MAX).cycle().take(1000).collect();
        round_trip(&data, "binary");
    }

    #[test]
    fn round_trip_ff_runs() {
        let mut data = vec![0xFFu8; 1024];
        data.extend_from_slice(&vec![0x00u8; 512]);
        data.extend_from_slice(&vec![0xFFu8; 512]);
        round_trip(&data, "ff-runs");
    }

    #[test]
    fn round_trip_repeated_phrase() {
        let phrase = b"all good coders write tests ";
        let data = phrase.repeat(64);
        round_trip(&data, "repeated-phrase");
    }

    #[test]
    fn compresses_repetitive_text() {
        // Use a longer input so the adaptive model has time to learn.
        let data = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".repeat(20);
        let mut enc = ArithmeticEncoder::new();
        let mut model = Order2Model::new();
        for &b in &data {
            model.encode_byte(b, &mut enc);
        }
        let bytes = enc.finish();
        assert!(
            bytes.len() < data.len(),
            "expected compression but got {} bytes for {} input",
            bytes.len(),
            data.len()
        );
    }

    #[test]
    fn compression_ratio_on_text() {
        let text = b"The quick brown fox jumps over the lazy dog. ".repeat(50);
        let mut enc = ArithmeticEncoder::new();
        let mut model = Order2Model::new();
        for &b in &text {
            model.encode_byte(b, &mut enc);
        }
        let bytes = enc.finish();
        let ratio = bytes.len() as f64 / text.len() as f64;
        eprintln!(
            "text: {} bytes -> {} bytes (ratio {:.3})",
            text.len(),
            bytes.len(),
            ratio
        );
        assert!(bytes.len() < text.len(), "expected compression");
    }
}
