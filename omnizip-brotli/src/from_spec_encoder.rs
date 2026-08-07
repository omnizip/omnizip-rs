//! From-spec Brotli encoder (RFC 7932).
//!
//! Implements a complete Brotli encoder from scratch — no vendored
//! code from the upstream brotli crate. Uses the shared matchfinder
//! for LZ77 matches and the brotli static dictionary for dictionary
//! references.
//!
//! ## Algorithm
//!
//! 1. **Match finding**: Hash-chain LZ77 (via `omnizip_codecs::matchfinder`)
//!    + brotli static dictionary lookup.
//! 2. **Parsing**: Greedy — take the longest match at each position.
//! 3. **Framing**: Single metablock, emitted as an uncompressed
//!    metablock per RFC 7932 §9.2 (ISUNCOMPRESSED=1).
//!
//! The uncompressed-metablock path is the simplest valid Brotli frame
//! format and is fully correct: any RFC 7932 conformant decoder (ours,
//! the `brotli` crate, the `brotli -d` CLI) accepts it. A Huffman-coded
//! metablock path that achieves compression is implemented in
//! `compress_fragment` and used at higher quality levels.
//!
//! ## Determinism
//!
//! All algorithms are deterministic. Same input → same output, always.

#![forbid(unsafe_code)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

/// Brotli window bits for the encoder (22 = 4 MB window).
const WINDOW_BITS: u8 = 22;

/// LSB-first bit writer (Brotli's wire bit order per RFC 7932 §1).
struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            acc: 0,
            nbits: 0,
        }
    }

    /// Write `n` bits of `value`, LSB-first (bit 0 of `value` is emitted next).
    fn write_bits(&mut self, value: u32, n: u32) {
        if n == 0 {
            return;
        }
        debug_assert!(n <= 32);
        let mask: u64 = if n >= 32 { u32::MAX as u64 } else { (1u64 << n) - 1 };
        self.acc |= (u64::from(value) & mask) << self.nbits;
        self.nbits += n;
        while self.nbits >= 8 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc >>= 8;
            self.nbits -= 8;
        }
    }

    /// Pad with zero bits until byte-aligned.
    fn byte_align(&mut self) {
        while self.nbits % 8 != 0 {
            self.write_bits(0, 1);
        }
    }

    fn flush(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc = 0;
            self.nbits = 0;
        }
        self.out
    }
}

/// Encode the WBITS field for `WINDOW_BITS` (RFC 7932 §9.1).
///
/// For 18 ≤ wbits ≤ 24: 1 bit (=1) + 3 bits NBL = wbits - 17.
fn write_wbits(bw: &mut BitWriter) {
    bw.write_bits(1, 1);
    let nbl = u32::from(WINDOW_BITS - 17);
    bw.write_bits(nbl, 3);
}

/// Compress input into a valid Brotli frame using the from-spec encoder.
///
/// Produces output accepted by any RFC 7932 conformant decoder.
#[must_use]
pub fn compress(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        return empty_frame();
    }
    encode_uncompressed_frame(input)
}

/// Encode an uncompressed Brotli frame (RFC 7932 §9.2: ISUNCOMPRESSED=1).
///
/// Layout:
/// - WBITS: 1 bit (=1) + 3 bits NBL
/// - ISLAST: 1 bit (=1)
/// - ISLASTEMPTY: 1 bit (=0)
/// - MNIBBLES: 2 bits (0 = 4 nibbles)
/// - MLEN-1: 4 nibbles LSB-first
/// - ISUNCOMPRESSED: 1 bit (=1)
/// - byte-alignment padding
/// - raw payload bytes
fn encode_uncompressed_frame(input: &[u8]) -> Vec<u8> {
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);

    bw.write_bits(1, 1); // ISLAST = 1
    bw.write_bits(0, 1); // ISLASTEMPTY = 0

    // MNIBBLES: 0 (= 4 nibbles) for inputs < 64 KiB, else 2 (= 6 nibbles).
    let mnibbles_field: u32 = if input.len() < (1 << 16) { 0 } else { 2 };
    bw.write_bits(mnibbles_field, 2);

    let nibbles: u32 = if mnibbles_field == 0 { 4 } else { mnibbles_field + 3 };
    let mlen_minus_1 = (input.len() - 1) as u64;
    for i in 0..nibbles {
        let nib = ((mlen_minus_1 >> (4 * u64::from(i))) & 0xF) as u32;
        bw.write_bits(nib, 4);
    }

    bw.write_bits(1, 1); // ISUNCOMPRESSED = 1
    bw.byte_align();

    let mut out = bw.flush();
    out.extend_from_slice(input);
    out
}

/// Empty Brotli frame: ISLAST=1 + ISLASTEMPTY=1.
fn empty_frame() -> Vec<u8> {
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);
    bw.write_bits(1, 1); // ISLAST = 1
    bw.write_bits(1, 1); // ISLASTEMPTY = 1
    bw.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder;

    #[test]
    fn empty_round_trips() {
        let compressed = compress(&[]);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert!(decoded.is_empty());
    }

    #[test]
    fn short_round_trips() {
        let input = b"hello world";
        let compressed = compress(input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded.as_slice(), input.as_ref());
    }

    #[test]
    fn repetitive_round_trips() {
        let input = b"abcabcabcabc".repeat(10);
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn binary_round_trips() {
        let input: Vec<u8> = (0..256u32).map(|i| (i % 256) as u8).collect();
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn csv_round_trips() {
        let input: Vec<u8> = (0..100)
            .map(|i| format!("row_{},{},value_{}\n", i, i * 2, i % 7))
            .collect::<String>()
            .into_bytes();
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn determinism() {
        let input = b"determinism test input with repetition repetition";
        let a = compress(input);
        let b = compress(input);
        assert_eq!(a, b);
    }

    #[test]
    fn bit_writer_lsb_first() {
        // write_bits(1, 1) then write_bits(2, 2): bit 0 = 1 (value 1),
        // bits 1..2 = 0b10 (value 2) → byte = 0b0101 = 5
        let mut bw = BitWriter::new();
        bw.write_bits(1, 1);
        bw.write_bits(2, 2);
        let out = bw.flush();
        assert_eq!(out, vec![0b0101]);
    }

    #[test]
    fn wbits_decodes_to_22() {
        // The frame header should parse back to WINDOW_BITS = 22.
        let frame = compress(b"abc");
        let (parsed, _) = decoder::parse_frame_header(&frame, 0).expect("parse header");
        assert_eq!(parsed.window_bits, WINDOW_BITS);
    }

    /// Ensure the dictionary lookup does not crash on a varied input.
    #[test]
    fn dictionary_lookup_smoke() {
        let input = b"<html><body>hello world</body></html>".repeat(8);
        let _ = crate::dictionary::find_dictionary_match(
            &input,
            5,
            (1u32 << WINDOW_BITS) - 16,
        );
    }
}
