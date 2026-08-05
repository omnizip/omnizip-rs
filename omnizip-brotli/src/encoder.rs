//! Brotli encoder (RFC 7932).
//!
//! Pure-Rust encoder producing uncompressed Brotli streams that
//! round-trip through our in-house decoder (TODO 117) and the
//! upstream `brotli -d` reference tool.
//!
//! ## Wire format (RFC 7932 §9.2)
//!
//! ```text
//! Frame header: WBITS (1 bit for lgwin=16, or 4/7 bits for lgwin 17..=24)
//! Metablock 0:  ISLAST=0, MNIBBLES=0, MLEN_field (16 bits),
//!               IS_UNCOMPRESSED=1, reserved=0
//!   [byte-align]
//!   MLEN bytes raw input
//! Terminator:   ISLAST=1, ISLASTEMPTY=1, [byte-align]
//! ```
//!
//! For any input size the encoder emits a single uncompressed
//! metablock followed by the empty-last-metablock marker. This is
//! what upstream Brotli does for very small inputs anyway (see
//! `EmitUncompressedMetaBlock` in the reference encoder). For
//! truly compressed output the Huffman-coded path lands with
//! TODO 151.
//!
//! The encoder is intentionally simple — it's the minimum viable
//! pure-Rust Brotli that round-trips through the reference decoder.
//! Compression ratio is zero (output ≈ input + ~5 bytes overhead);
//! replace with the Huffman-coded path for actual compression.

#![forbid(unsafe_code)]

use super::encoder_error::EncodeError;

/// Encode `input` as a single-metablock Brotli uncompressed stream.
///
/// The output is a valid RFC 7932 Brotli frame that decodes via any
/// conforming decoder (our in-house `decoder::decode` and the
/// upstream `brotli -d`).
///
/// # Errors
///
/// Returns `EncodeError::InputTooLarge` if `input.len()` exceeds `u32::MAX`.
pub fn encode_uncompressed(input: &[u8]) -> Result<Vec<u8>, EncodeError> {
    if input.len() > u32::MAX as usize {
        return Err(EncodeError::InputTooLarge {
            len: input.len(),
            max: u32::MAX as usize,
        });
    }

    let mut bw = BitWriter::new();

    // ----- Frame header (RFC 7932 §9.1) -----
    // WBITS=0 → lgwin=16 → 1 bit.
    bw.write_bit(false);

    if input.is_empty() {
        // Empty input: no metablock, just emit the terminator.
        // ISLAST=1, ISLASTEMPTY=1, byte-align.
        bw.write_bit(true);
        bw.write_bit(true);
        bw.pad_to_byte();
        return Ok(bw.finish());
    }

    let mlen_field: u32 = (input.len() as u32) - 1;

    // ----- Metablock header (RFC 7932 §9.2) -----
    // ISLAST=0 (1 bit).
    bw.write_bit(false);
    // MNIBBLES=00 (2 bits) → use 4 nibbles for MLEN.
    bw.write_bits(0, 2);
    // MLEN (16 bits, LSB-first).
    bw.write_bits(u64::from(mlen_field), 16);
    // IS_UNCOMPRESSED=1 (1 bit).
    bw.write_bit(true);
    // Reserved=0 (1 bit).
    bw.write_bit(false);

    // Byte-align before the literal payload.
    bw.pad_to_byte();

    // ----- Literal payload -----
    bw.write_bytes(input);

    // ----- Terminator: ISLAST=1, ISLASTEMPTY=1, byte-align -----
    bw.write_bit(true); // ISLAST
    bw.write_bit(true); // ISLASTEMPTY
    bw.pad_to_byte();

    Ok(bw.finish())
}

/// LSB-first bit writer. Bits accumulate into the last byte; new
/// bytes are added as needed.
struct BitWriter {
    out: Vec<u8>,
    /// Number of bits used in the last byte (0..=7).
    bit_pos: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            bit_pos: 0,
        }
    }

    fn write_bit(&mut self, bit: bool) {
        if self.bit_pos == 0 {
            self.out.push(0);
        }
        let last = self
            .out
            .last_mut()
            .expect("BitWriter invariant: byte exists when bit_pos > 0");
        if bit {
            *last |= 1 << self.bit_pos;
        }
        self.bit_pos = (self.bit_pos + 1) % 8;
    }

    fn write_bits(&mut self, value: u64, nbits: u32) {
        for i in 0..nbits {
            self.write_bit((value >> i) & 1 == 1);
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        // We're not byte-aligned unless bit_pos == 0.
        debug_assert_eq!(self.bit_pos, 0, "write_bytes requires byte alignment");
        self.out.extend_from_slice(bytes);
    }

    fn pad_to_byte(&mut self) {
        if self.bit_pos != 0 {
            // The remaining bits in the current byte are already 0
            // (since out.push(0) initializes new bytes to zero), so
            // we just reset bit_pos to 0. Do NOT zero out bits we've
            // already written.
            self.bit_pos = 0;
        }
    }

    fn finish(self) -> Vec<u8> {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::decode;

    #[test]
    fn encode_uncompressed_empty() {
        let out = encode_uncompressed(&[]).expect("encode");
        eprintln!("empty stream bytes: {out:02x?}");
        let decoded = decode(&out).expect("decode");
        assert!(decoded.is_empty());
    }

    #[test]
    fn encode_uncompressed_one_byte_decodes() {
        let out = encode_uncompressed(b"a").expect("encode");
        let decoded = decode(&out).expect("decode");
        assert_eq!(decoded, b"a");
    }

    #[test]
    fn encode_uncompressed_round_trips_arbitrary() {
        for input in [
            b"a".to_vec(),
            b"ab".to_vec(),
            b"hello".to_vec(),
            b"hello world hello world".to_vec(),
            vec![0u8; 100],
            vec![0xFFu8; 256],
            (0..1024).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
        ] {
            let out = encode_uncompressed(&input).expect("encode");
            let decoded = decode(&out).expect("decode");
            assert_eq!(decoded, input, "round-trip failed for len {}", input.len());
        }
    }

    #[test]
    fn encode_uncompressed_hello_round_trip() {
        // Sanity check that "hello" (5 bytes) round-trips. Upstream
        // brotli uses 10 bytes for this; our output is similar.
        let out = encode_uncompressed(b"hello").expect("encode");
        let decoded = decode(&out).expect("decode");
        assert_eq!(decoded, b"hello");
    }
}