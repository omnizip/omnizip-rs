//! Deflate64 decoder: Huffman decode + LZ77 reconstruction.
//!
//! Direct port of:
//! - `omnizip/lib/omnizip/algorithms/deflate64/decoder.rb`
//! - the decode half of `huffman_coder.rb`
//!
//! Reverses [`crate::encoder`]: unpacks the bitstream into LZ77 tokens via
//! the serialised Huffman tables, then reconstructs the original bytes from
//! the 64 KB sliding window.

#![allow(clippy::cast_possible_truncation)]

use crate::constants::END_OF_BLOCK;
use crate::huffman::{
    distance_decode, distance_extra_bits, length_decode, length_extra_bits, HuffTable, InverseTable,
};
use crate::token::Token;

/// A Deflate64 decoder operating on already-separated Huffman tables and
/// bitstream. [`crate::container`] splits the container into these parts.
pub struct Decoder;

impl Decoder {
    /// Decode the bitstream into LZ77 tokens using the supplied tables,
    /// then reconstruct the original bytes.
    ///
    /// `expected_len` is the length the output must have; the decoder
    /// returns `Err` if it cannot consume exactly that many bytes.
    ///
    /// # Errors
    ///
    /// - [`DecodeError::Corrupt`] if the bitstream or tables are malformed.
    /// - [`DecodeError::LengthMismatch`] if the reconstructed length is wrong.
    pub fn decode(
        lit_table: &HuffTable,
        dist_table: &HuffTable,
        bitstream: &[u8],
        expected_len: usize,
    ) -> Result<Vec<u8>, DecodeError> {
        let tokens = decode_tokens(lit_table, dist_table, bitstream)?;
        let out = reconstruct_from_tokens(&tokens);
        if out.len() != expected_len {
            return Err(DecodeError::LengthMismatch {
                expected: expected_len,
                actual: out.len(),
            });
        }
        Ok(out)
    }
}

/// Errors raised during Deflate64 decode.
#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The bitstream or tables could not be parsed.
    Corrupt {
        /// Human-readable detail.
        reason: String,
    },
    /// The output length did not match `expected_len`.
    LengthMismatch {
        /// Expected length from the drop record.
        expected: usize,
        /// Actual length the decoder produced.
        actual: usize,
    },
}

/// Expand the packed byte bitstream into a flat vector of 0/1 bits.
fn bytes_to_bits(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 8);
    for &byte in bytes {
        for i in (0..8).rev() {
            out.push((byte >> i) & 1);
        }
    }
    out
}

/// Decode the token stream — port of `HuffmanCoder#decode_tokens`.
fn decode_tokens(
    lit_table: &HuffTable,
    dist_table: &HuffTable,
    bitstream: &[u8],
) -> Result<Vec<Token>, DecodeError> {
    let mut tokens = Vec::new();
    let bits = bytes_to_bits(bitstream);
    let lit_inv = lit_table.invert();
    let dist_inv = dist_table.invert();

    let mut pos = 0usize;
    while pos < bits.len() {
        let symbol = decode_one_symbol(&lit_inv, &bits, &mut pos)?;
        if symbol == END_OF_BLOCK {
            break;
        }
        if symbol < 256 {
            tokens.push(Token::Literal {
                value: symbol as u8,
            });
        } else {
            let len_xbits = length_extra_bits(symbol);
            let len_extra = read_extra(&bits, &mut pos, len_xbits)?;
            let length = length_decode(symbol, len_extra);

            let dist_symbol = decode_one_symbol(&dist_inv, &bits, &mut pos)?;
            let dist_xbits = distance_extra_bits(dist_symbol as u8);
            let dist_extra = read_extra(&bits, &mut pos, dist_xbits)?;
            let distance = distance_decode(dist_symbol as u8, dist_extra);
            tokens.push(Token::Match { length, distance });
        }
    }
    Ok(tokens)
}

/// Pull a single symbol from the bit stream via an inverse table.
fn decode_one_symbol(
    table: &InverseTable,
    bits: &[u8],
    pos: &mut usize,
) -> Result<u16, DecodeError> {
    table
        .decode_symbol(bits, pos)
        .ok_or_else(|| DecodeError::Corrupt {
            reason: format!("failed to decode symbol at bit position {pos}"),
        })
}

/// Read `count` extra bits LSB-first (RFC 1951 convention) and return the
/// assembled value. Returns 0 when `count` is 0.
fn read_extra(bits: &[u8], pos: &mut usize, count: u8) -> Result<u32, DecodeError> {
    if count == 0 {
        return Ok(0);
    }
    let end = *pos + usize::from(count);
    if end > bits.len() {
        return Err(DecodeError::Corrupt {
            reason: format!("extra bits underflow at bit position {pos}"),
        });
    }
    let mut value: u32 = 0;
    for i in 0..usize::from(count) {
        value |= u32::from(bits[*pos + i]) << i;
    }
    *pos = end;
    Ok(value)
}

/// Reconstruct the original bytes from LZ77 tokens via a sliding window.
///
/// Port of `Decoder#reconstruct_from_tokens` + `#copy_from_window`. The
/// window is bounded to the Deflate64 dictionary size; entries beyond it
/// are dropped from the front.
fn reconstruct_from_tokens(tokens: &[Token]) -> Vec<u8> {
    let window_size = crate::constants::DICTIONARY_SIZE;
    let mut out: Vec<u8> = Vec::new();
    let mut window: Vec<u8> = Vec::with_capacity(window_size);

    for token in tokens {
        match *token {
            Token::Literal { value } => {
                out.push(value);
                push_window(&mut window, value, window_size);
            }
            Token::Match { distance, length } => {
                copy_from_window(&mut out, &mut window, distance, length, window_size);
            }
        }
    }
    out
}

/// Push a byte onto the window, evicting the oldest if over capacity.
fn push_window(window: &mut Vec<u8>, value: u8, window_size: usize) {
    window.push(value);
    if window.len() > window_size {
        window.remove(0);
    }
}

/// Copy `length` bytes from `distance` back in the window, supporting
/// run-length copies where `length > distance`.
fn copy_from_window(
    out: &mut Vec<u8>,
    window: &mut Vec<u8>,
    distance: usize,
    length: usize,
    window_size: usize,
) {
    if distance == 0 || distance > window.len() {
        return;
    }
    let start = window.len() - distance;
    for i in 0..length {
        let byte = window[start + (i % distance)];
        out.push(byte);
        push_window(window, byte, window_size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::Encoder;

    fn round_trip(data: &[u8]) {
        let enc = Encoder::new();
        let encoded = enc.encode(data);
        let decoded = Decoder::decode(
            &encoded.literal_table,
            &encoded.distance_table,
            &encoded.bitstream,
            data.len(),
        )
        .expect("decode");
        assert_eq!(decoded, data, "round-trip mismatch for len {}", data.len());
    }

    #[test]
    fn round_trip_empty() {
        round_trip(b"");
    }

    #[test]
    fn round_trip_short() {
        round_trip(b"Hi");
    }

    #[test]
    fn round_trip_repetitive() {
        let data = b"ABCABCABCABCABCABC".repeat(20);
        round_trip(&data);
    }

    #[test]
    fn length_mismatch_detected() {
        let enc = Encoder::new();
        let encoded = enc.encode(b"hello");
        let err = Decoder::decode(
            &encoded.literal_table,
            &encoded.distance_table,
            &encoded.bitstream,
            100,
        );
        assert_eq!(
            err,
            Err(DecodeError::LengthMismatch {
                expected: 100,
                actual: 5
            })
        );
    }
}
