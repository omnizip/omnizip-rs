//! Pure-Rust RFC 1951 DEFLATE encoder (Phase 3, minimal).
//!
//! Implements **stored blocks only** — no compression, just byte-
//! for-byte copy. Round-trips through any RFC 1951 decoder.
//!
//! This is the minimal in-house encode path. A future optimisation
//! pass would add fixed-Huffman + simple LZ77 (target: within 5% of
//! `zlib -6`). See TODO 104 Phase 3 follow-up.
//!
//! ## Stored block format
//!
//! ```text
//! BFINAL    1 bit   = 1 (this is the last block)
//! BTYPE     2 bits  = 00 (stored)
//! <byte align>
//! LEN       2 bytes (little-endian; payload size)
//! NLEN      2 bytes (= !LEN, sanity check)
//! payload   LEN raw bytes
//! ```
//!
//! For inputs > 65535 bytes, multiple stored blocks are chained
//! (BFINAL=0 on all but the last).

#![forbid(unsafe_code)]

use omnizip_codecs::{CodecId, OmnizipError};

/// Maximum payload per stored block (RFC 1951 limit).
const MAX_STORED_LEN: usize = 0xFFFF;

/// Encode `input` as a series of RFC 1951 stored blocks.
///
/// Output is raw DEFLATE (no zlib/gzip wrapper).
///
/// # Errors
///
/// Returns [`OmnizipError::Corrupt`] only on arithmetic overflow
/// (shouldn't happen for any input).
pub fn deflate_stored(input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    let mut out = Vec::with_capacity(input.len() + 16);

    if input.is_empty() {
        // Single empty final stored block.
        // Bits: BFINAL=1, BTYPE=00 → 0b001 = 1.
        out.push(0x01);
        out.extend_from_slice(&[0x00, 0x00]); // LEN = 0
        out.extend_from_slice(&[0xFF, 0xFF]); // NLEN = 0xFFFF
        return Ok(out);
    }

    let mut offset = 0;
    while offset < input.len() {
        let remaining = input.len() - offset;
        let chunk_size = remaining.min(MAX_STORED_LEN);
        let is_final = offset + chunk_size == input.len();

        // Header byte: bit 0 = BFINAL, bits 1-2 = BTYPE.
        let header_byte: u8 = if is_final { 0b001 } else { 0b000 };
        out.push(header_byte);

        // Byte alignment: since BFINAL + BTYPE consumed 3 bits and
        // we just wrote a full byte, we're already aligned. Pad bits
        // are implicitly zero (the bit writer would have written
        // them in the low bits of this byte).

        // LEN + NLEN (little-endian).
        let len = u16::try_from(chunk_size).unwrap_or(0);
        let nlen = !len;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&nlen.to_le_bytes());

        // Payload.
        out.extend_from_slice(&input[offset..offset + chunk_size]);
        offset += chunk_size;
    }

    let _ = CodecId::LIBDEFLATE;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_round_trips_via_inflate() {
        let compressed = deflate_stored(b"").unwrap();
        let decoded = crate::inflate::inflate(&compressed, 0).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn small_input_round_trips_via_inflate() {
        let input = b"hello world";
        let compressed = deflate_stored(input).unwrap();
        let decoded = crate::inflate::inflate(&compressed, input.len()).unwrap();
        assert_eq!(decoded, input);
    }

    #[test]
    fn multi_block_input_round_trips() {
        // 100_000 bytes forces two stored blocks (MAX_STORED_LEN = 65535).
        let input: Vec<u8> = (0..100_000).map(|i| (i & 0xFF) as u8).collect();
        let compressed = deflate_stored(&input).unwrap();
        let decoded = crate::inflate::inflate(&compressed, input.len()).unwrap();
        assert_eq!(decoded, input);
    }

    #[test]
    fn exactly_max_len_round_trips() {
        let input: Vec<u8> = vec![0x42; MAX_STORED_LEN];
        let compressed = deflate_stored(&input).unwrap();
        let decoded = crate::inflate::inflate(&compressed, input.len()).unwrap();
        assert_eq!(decoded, input);
    }
}
