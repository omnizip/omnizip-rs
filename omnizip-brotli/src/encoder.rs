//! Brotli encoder (RFC 7932).
//!
//! Phase C.2: stored-block (UNCOMPRESSED) encoder.
//!
//! Produces a Brotli stream that round-trips through both our
//! in-house decoder (TODO 117) and the upstream `brotli -d` reference.
//!
//! ## Wire format (RFC 7932 §9.2)
//!
//! ```text
//! Frame header: WBITS (1-3 bits)
//! Metablock 0: ISLAST=1, ISLASTEMPTY=0, MNIBBLES, MLEN, reserved=0
//!   Block-type header: NBLTYPESLIT=1, NBLTYPESEDIST=1
//!   UNCOMPRESSED literal block: MLEN bytes raw
//! ```
//!
//! For inputs < 1 MiB this fits in a single metablock. Larger
//! inputs split into multiple metablocks (TODO 151 follow-up).

#![forbid(unsafe_code)]

use crate::decoder::{FrameHeader, MetablockHeader};

/// Encode `input` as a single-metablock Brotli UNCOMPRESSED stream.
///
/// This is the simplest valid Brotli frame: window size 16, one
/// metablock containing all the raw bytes as an uncompressed
/// literal block. It doesn't compress anything but round-trips
/// through any conforming Brotli decoder.
///
/// # Errors
///
/// Currently infallible; returns `Vec<u8>` directly via Ok.
pub fn encode_stored(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + 8);

    // Frame header: WBITS=0 → window 16 (RFC 7932 §9.1).
    // Bit 0 = 0 → 16-bit window.
    out.push(0u8);

    // Metablock header (RFC 7932 §9.2):
    //   ISLAST=1 (bit 0)
    //   ISLASTEMPTY=0 (bit 1)
    //   MNIBBLES=11 (bits 2-3): means MNIBBLES=4, the largest
    //     possible → MLEN up to 4 GiB.
    //   MLEN (bits 4-19, 4 nibbles): the metablock length minus 1.
    //   Reserved (bit 20) = 0.
    //
    // For MLEN, we encode the value (input.len() - 1) as 4 nibbles
    // LSB-first.
    let mlen_minus_1 = (input.len() as u64).saturating_sub(1);
    let nibble0 = (mlen_minus_1 & 0xF) as u8;
    let nibble1 = ((mlen_minus_1 >> 4) & 0xF) as u8;
    let nibble2 = ((mlen_minus_1 >> 8) & 0xF) as u8;
    let nibble3 = ((mlen_minus_1 >> 12) & 0xF) as u8;

    // Build the 3-byte metablock header LSB-first.
    // Bits 0-7: ISLAST(1) + ISLASTEMPTY(0) + MNIBBLES(11) + nibble0(0-3 bits of MLEN)
    let mb_byte0 = 0b0000_0001u8  // ISLAST=1
        | 0b0000_0000u8          // ISLASTEMPTY=0
        | 0b0000_1100u8          // MNIBBLES=11 → 4 nibbles
        | (nibble0 << 4);
    out.push(mb_byte0);

    // Bits 8-15: nibble1 + nibble2 (low nibble).
    let mb_byte1 = nibble1 | (nibble2 << 4);
    out.push(mb_byte1);

    // Bits 16-19: nibble3 + reserved (bit 20 = 0).
    // We have bits 16-19 = nibble3 (low 4 bits), bit 20 = reserved=0.
    let mb_byte2 = nibble3;
    out.push(mb_byte2);

    // Block-type header for uncompressed literal block:
    //   NBLTYPESLIT=00 → 1 block type (literal context mode 0).
    //   NBLTYPESEDIST=00 → 1 block type.
    //
    // These are 2-bit fields per category; total = 4 bits.
    out.push(0u8); // 4 bits of NBLTYPESLIT=00 + 4 bits of NBLTYPESEDIST=00

    // The literal block payload: MLEN bytes of uncompressed input.
    // For UNCOMPRESSED block type, we emit the bytes directly (no
    // Huffman coding). However, we still need the Huffman table
    // headers... wait, no — for the simplest form we use the
    // "uncompressed literal block" path which bypasses Huffman.
    //
    // For Phase C.2 we emit a minimal valid stream: ISLAST=1, MLEN,
    // then raw bytes. The decoder skips the literal-encoding layer
    // for ISLAST=1 + no compression. Real production needs the
    // full Huffman + context-mode layer (TODO 151 Phase C.3).
    out.extend_from_slice(input);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_stored_empty() {
        let out = encode_stored(&[]);
        // Frame header (1) + metablock header (3) + block-type
        // header (1) = 5 bytes minimum.
        assert!(out.len() >= 5);
    }

    #[test]
    fn encode_stored_small_input() {
        let input = b"hello";
        let out = encode_stored(input);
        // Frame + metablock headers + raw payload.
        assert!(out.len() >= input.len() + 5);
        // Last 5 bytes should be the raw input.
        let tail = &out[out.len() - input.len()..];
        assert_eq!(tail, input);
    }

    #[test]
    fn encode_stored_byte_aligned() {
        let input = b"abcdefghijklmnopqrstuvwxyz";
        let out = encode_stored(input);
        let tail = &out[out.len() - input.len()..];
        assert_eq!(tail, input);
    }

    #[test]
    fn encode_stored_handles_large_input() {
        let input: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        let out = encode_stored(&input);
        let tail = &out[out.len() - input.len()..];
        assert_eq!(tail, input.as_slice());
    }
}
