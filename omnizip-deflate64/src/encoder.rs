//! Deflate64 encoder: LZ77 match finding + Huffman coding + container framing.
//!
//! Direct port of:
//! - `omnizip/lib/omnizip/algorithms/deflate64/lz77_encoder.rb`
//! - `omnizip/lib/omnizip/algorithms/deflate64/encoder.rb`
//! - the encode half of `huffman_coder.rb`
//!
//! The container format is the Ruby reference's: a header carrying the two
//! Huffman tables, followed by the Huffman-coded token bitstream padded to a
//! byte boundary. The format is self-describing and deterministic.

#![allow(clippy::cast_possible_truncation)]

use crate::constants::{
    END_OF_BLOCK, HASH_SHIFT, HASH_SIZE, MAX_CHAIN_LENGTH, MAX_DISTANCE, MAX_MATCH_LENGTH,
    MIN_MATCH_LENGTH, NICE_MATCH,
};
use crate::huffman::{distance_encode, length_encode, HuffCode, HuffTable};
use crate::token::Token;

/// LZ77 + Huffman encoder for Deflate64.
pub struct Encoder {
    /// Sliding-window size (64 KB for Deflate64).
    window_size: usize,
}

impl Encoder {
    /// Create an encoder with the standard 64 KB Deflate64 window.
    #[must_use]
    pub fn new() -> Self {
        Self {
            window_size: crate::constants::DICTIONARY_SIZE,
        }
    }

    /// Compress `data` into the self-describing Deflate64 container.
    ///
    /// Returns `(literal_table, distance_table, compressed_bitstream_bytes)`.
    /// The caller wraps these into the final on-the-wire container.
    #[must_use]
    pub fn encode(&self, data: &[u8]) -> Encoded {
        let tokens = self.find_matches(data);
        let (lit_table, dist_table, bitstream) = Self::huffman_encode(&tokens);
        Encoded {
            literal_table: lit_table,
            distance_table: dist_table,
            bitstream,
        }
    }

    /// LZ77 match finding — port of `LZ77Encoder#find_matches`.
    fn find_matches(&self, data: &[u8]) -> Vec<Token> {
        let mut tokens = Vec::new();
        // Hash chain: `head[hash]` = most recent position with that hash;
        // `prev[pos % window]` = previous position in the same hash chain.
        let mut head = vec![usize::MAX; HASH_SIZE];
        let mut prev = vec![usize::MAX; self.window_size];

        let mut pos = 0usize;
        while pos < data.len() {
            if let Some((length, distance)) = self.find_longest_match(data, pos, &head, &prev) {
                if length >= MIN_MATCH_LENGTH {
                    tokens.push(Token::Match { length, distance });
                    // Insert hash entries for every position inside the match
                    // so future matches can reference them.
                    for i in 0..length {
                        let p = pos + i;
                        if p + MIN_MATCH_LENGTH <= data.len() {
                            insert_hash(data, p, &mut head, &mut prev, self.window_size);
                        }
                    }
                    pos += length;
                    continue;
                }
            }
            tokens.push(Token::Literal { value: data[pos] });
            if pos + MIN_MATCH_LENGTH <= data.len() {
                insert_hash(data, pos, &mut head, &mut prev, self.window_size);
            }
            pos += 1;
        }
        tokens
    }

    /// Find the longest match at `pos` using the hash chain.
    ///
    /// Port of `LZ77Encoder#find_longest_match`. Returns the best
    /// `(length, distance)` or `None`.
    fn find_longest_match(
        &self,
        data: &[u8],
        pos: usize,
        head: &[usize],
        prev: &[usize],
    ) -> Option<(usize, usize)> {
        if pos + MIN_MATCH_LENGTH > data.len() {
            return None;
        }
        let hash = calculate_hash(data, pos);
        let mut candidate = head[hash];
        if candidate == usize::MAX {
            return None;
        }

        let mut best_length = MIN_MATCH_LENGTH - 1;
        let mut best_match: Option<(usize, usize)> = None;
        let max_length = (MAX_MATCH_LENGTH).min(data.len() - pos);
        let mut chain = 0usize;

        while candidate != usize::MAX && chain < MAX_CHAIN_LENGTH {
            let distance = pos - candidate;
            if distance > MAX_DISTANCE {
                break;
            }
            // Quick reject: compare the byte at best_length to avoid wasted work.
            if best_length > 0 && data.get(pos + best_length) != data.get(candidate + best_length) {
                candidate = prev_next(prev, candidate, self.window_size);
                chain += 1;
                continue;
            }
            let length = match_length(data, pos, candidate, max_length);
            if length > best_length {
                best_length = length;
                best_match = Some((length, distance));
                if length >= NICE_MATCH {
                    break;
                }
            }
            candidate = prev_next(prev, candidate, self.window_size);
            chain += 1;
        }

        best_match
    }

    /// Build frequency tables, construct Huffman trees, emit the bitstream.
    fn huffman_encode(tokens: &[Token]) -> (HuffTable, HuffTable, Vec<u8>) {
        // Build literal/length and distance frequency tables.
        let mut lit_freqs: Vec<(u16, u64)> = Vec::new();
        let mut dist_freqs: Vec<(u16, u64)> = Vec::new();

        // Use a dense array for frequencies (symbols 0..=285), then collect.
        let mut lit_counts = vec![0u64; 286];
        let mut dist_counts = vec![0u64; 30];

        for token in tokens {
            match *token {
                Token::Literal { value } => {
                    lit_counts[usize::from(value)] += 1;
                }
                Token::Match { length, distance } => {
                    let (len_code, _, _) = length_encode(length);
                    lit_counts[usize::from(len_code)] += 1;
                    let (dist_code, _, _) = distance_encode(distance);
                    dist_counts[usize::from(dist_code)] += 1;
                }
            }
        }
        // End-of-block marker always appears once.
        lit_counts[usize::from(END_OF_BLOCK)] += 1;

        for (sym, &freq) in lit_counts.iter().enumerate() {
            if freq > 0 {
                lit_freqs.push((sym as u16, freq));
            }
        }
        for (sym, &freq) in dist_counts.iter().enumerate() {
            if freq > 0 {
                dist_freqs.push((sym as u16, freq));
            }
        }

        let lit_table = HuffTable::from_frequencies(&lit_freqs);
        let dist_table = HuffTable::from_frequencies(&dist_freqs);

        let bits = encode_tokens(tokens, &lit_table, &dist_table);
        (lit_table, dist_table, bits_to_bytes(&bits))
    }
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

/// The output of a successful encode: the two Huffman tables and the
/// bitstream. The [`crate::container`] module packs these into bytes.
pub struct Encoded {
    /// Literal/length Huffman table.
    pub literal_table: HuffTable,
    /// Distance Huffman table.
    pub distance_table: HuffTable,
    /// Huffman-coded token bitstream, padded to a byte boundary.
    pub bitstream: Vec<u8>,
}

/// Encode tokens to a `Vec<u8>` of bit values (each byte is 0 or 1).
fn encode_tokens(tokens: &[Token], lit: &HuffTable, dist: &HuffTable) -> Vec<u8> {
    let mut bits = Vec::new();
    for token in tokens {
        match *token {
            Token::Literal { value } => {
                if let Some(code) = lit.code_for(u16::from(value)) {
                    push_code(&mut bits, code);
                }
            }
            Token::Match { length, distance } => {
                let (len_code, len_extra, len_extra_bits) = length_encode(length);
                if let Some(code) = lit.code_for(len_code) {
                    push_code(&mut bits, code);
                }
                // Length extra bits: LSB-first per RFC 1951.
                push_extra(&mut bits, len_extra, len_extra_bits);
                let (dist_code, dist_extra, dist_extra_bits) = distance_encode(distance);
                if let Some(code) = dist.code_for(u16::from(dist_code)) {
                    push_code(&mut bits, code);
                }
                push_extra(&mut bits, dist_extra, dist_extra_bits);
            }
        }
    }
    if let Some(code) = lit.code_for(END_OF_BLOCK) {
        push_code(&mut bits, code);
    }
    bits
}

/// Push the bits of a [`HuffCode`] MSB-first into the bit vector.
fn push_code(bits: &mut Vec<u8>, code: HuffCode) {
    if code.len == 0 {
        return;
    }
    for i in (0..code.len).rev() {
        bits.push(((code.bits >> i) & 1) as u8);
    }
}

/// Push `count` extra-bit value LSB-first (RFC 1951 convention).
fn push_extra(bits: &mut Vec<u8>, value: u32, count: u8) {
    for i in 0..count {
        bits.push(((value >> i) & 1) as u8);
    }
}

/// Convert a flat bit vector (0/1 per byte) into packed bytes, padding the
/// final byte with zeros. Port of Ruby `bits_to_bytes`.
fn bits_to_bytes(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bits.len().div_ceil(8));
    let mut i = 0;
    while i < bits.len() {
        let mut byte: u8 = 0;
        let end = (i + 8).min(bits.len());
        for (k, j) in (i..end).enumerate() {
            if bits[j] != 0 {
                byte |= 1 << (7 - k);
            }
        }
        out.push(byte);
        i = end;
    }
    out
}

/// Insert `pos` into the hash chain. Port of the bookkeeping the Ruby does
/// inside `find_longest_match` (the `@hash_table[hash] << pos` line).
fn insert_hash(data: &[u8], pos: usize, head: &mut [usize], prev: &mut [usize], window: usize) {
    let hash = calculate_hash(data, pos);
    prev[pos % window] = head[hash];
    head[hash] = pos;
}

/// Follow the hash chain backwards.
fn prev_next(prev: &[usize], candidate: usize, window: usize) -> usize {
    prev[candidate % window]
}

/// 3-byte rolling hash — port of `LZ77Encoder#calculate_hash`.
fn calculate_hash(data: &[u8], pos: usize) -> usize {
    if pos + MIN_MATCH_LENGTH > data.len() {
        return 0;
    }
    let mut hash: usize = 0;
    for i in 0..MIN_MATCH_LENGTH {
        hash = ((hash << HASH_SHIFT) ^ usize::from(data[pos + i])) & (HASH_SIZE - 1);
    }
    hash
}

/// Count matching bytes from two positions, capped at `max_length`.
fn match_length(data: &[u8], pos1: usize, pos2: usize, max_length: usize) -> usize {
    let mut length = 0;
    while length < max_length && data.get(pos1 + length) == data.get(pos2 + length) {
        length += 1;
    }
    length
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        let enc = Encoder::new();
        let out = enc.encode(b"");
        // No tokens; bitstream is just the end-of-block marker.
        assert!(out.bitstream.len() <= 2);
    }

    #[test]
    fn literals_only() {
        let enc = Encoder::new();
        let data = b"abcdef";
        let out = enc.encode(data);
        // Should contain only literal codes + EOB; the tables exist.
        assert!(out.literal_table.code_for(u16::from(b'a')).is_some());
    }
}
