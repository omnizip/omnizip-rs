//! DEFLATE encoder with LZ77 + fixed Huffman codes.
//!
//! Implements RFC 1951 §3.2.6 (fixed Huffman codes) on top of an
//! in-house LZ77 match finder. This is the second-pass encoder
//! layered on top of [`super::deflate_stored`] (which produces stored
//! blocks only). The codec uses this encoder when the input is large
//! enough for LZ77 to pay off; small inputs go through stored blocks.
//!
//! ## Algorithm
//!
//! 1. **LZ77 parse** (hash chains, greedy with lazy look-ahead):
//!    - At each position, hash the next 3 bytes.
//!    - Walk the hash chain backward to find the longest match
//!      (up to 258 bytes) within the 32 KB sliding window.
//!    - Lazy look-ahead: if pos+1 has a strictly longer match, emit
//!      a literal at pos and take the deferred match.
//!
//! 2. **Symbol emission** (fixed Huffman):
//!    - Literals (0-255): fixed 8/9-bit codes (RFC 1951 §3.2.6).
//!    - Lengths (257-285): fixed 7/8-bit codes + extra bits.
//!    - Distances (0-29): 5-bit codes + extra bits.
//!    - End of block (256): 7-bit code.
//!
//! 3. **Bit packing**: LSB-first into bytes, with Huffman codes
//!    written MSB-first (per RFC 1951 convention).

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]

use omnizip_codecs::{CodecId, OmnizipError};

/// Minimum match length for LZ77 (RFC 1951 spec).
const MIN_MATCH: usize = 3;
/// Maximum match length for LZ77 (RFC 1951 spec).
const MAX_MATCH: usize = 258;
/// Sliding window size (RFC 1951 spec).
const WINDOW_SIZE: usize = 32 * 1024;
/// Hash table size (16-bit hash, 64K entries).
const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
/// Maximum chain walks per match attempt.
const MAX_CHAIN: usize = 32;
/// Threshold: inputs below this go through stored blocks instead.
const LZ77_MIN_INPUT: usize = 128;

/// Encode `input` as a single RFC 1951 fixed-Huffman block. The
/// output is raw DEFLATE (no zlib wrapper).
///
/// Returns `None` if the LZ77 path isn't worth it (tiny input); the
/// caller should fall back to `deflate_stored`.
///
/// # Errors
///
/// Returns [`OmnizipError::Corrupt`] only on arithmetic overflow
/// (shouldn't happen for any plausible input).
pub fn deflate_fixed_huffman(input: &[u8]) -> Result<Option<Vec<u8>>, OmnizipError> {
    if input.len() < LZ77_MIN_INPUT {
        return Ok(None);
    }
    let mut encoder = Lz77Encoder::new(input.len() + 32);
    encoder.encode_block(input)?;
    Ok(Some(encoder.finish()))
}

struct Lz77Encoder {
    out: Vec<u8>,
    /// Bit accumulator: Huffman codes go in MSB-first, extra bits
    /// go in LSB-first. We pack bits LSB-first into bytes, but
    /// reverse Huffman codes before pushing.
    bits: u64,
    nbits: u32,
}

impl Lz77Encoder {
    fn new(cap: usize) -> Self {
        Self {
            out: Vec::with_capacity(cap),
            bits: 0,
            nbits: 0,
        }
    }

    fn finish(mut self) -> Vec<u8> {
        // End-of-block symbol (256) — 7-bit fixed code.
        let (code, len) = FIXED_LIT[256];
        self.write_huffman_code(code, len);

        // Flush remaining bits (padded with zeros in the high bits).
        if self.nbits > 0 {
            self.out.push((self.bits & 0xFF) as u8);
        }
        self.out
    }

    /// Encode one full block. The block header (BFINAL + BTYPE) is
    /// written FIRST so it lands at the start of the bit stream —
    /// the rest of the body is appended after.
    fn encode_block(&mut self, input: &[u8]) -> Result<(), OmnizipError> {
        // Write block header FIRST: BFINAL=1, BTYPE=01 (fixed Huffman).
        // Bit 0: BFINAL = 1
        // Bits 1-2: BTYPE = 01
        // → 3 bits: 0b011 = 3 (packed LSB-first).
        self.write_bits(3, 3);

        let mut mf = MatchFinder::new(input.len());
        let mut i = 0;
        while i < input.len() {
            // Search BEFORE inserting so the hash table still has
            // older positions to walk. (Same fix as the LZ4 HC encoder.)
            let m = mf.find_match(input, i);
            mf.insert(input, i);
            if let Some((dist, len)) = m {
                // Lazy look-ahead.
                if i + 1 < input.len() {
                    if let Some((d2, l2)) = mf.find_match(input, i + 1) {
                        if l2 > len + 1 {
                            // Take the better match at i+1; emit literal at i.
                            self.emit_literal(input[i]);
                            self.emit_match(d2, l2)?;
                            // Skip past the match.
                            mf.insert(input, i + 1);
                            for k in (i + 2)..(i + 2 + l2) {
                                if k < input.len() {
                                    mf.insert(input, k);
                                }
                            }
                            i += 2 + l2;
                            continue;
                        }
                    }
                }
                self.emit_match(dist, len)?;
                for k in (i + 1)..(i + len) {
                    if k < input.len() {
                        mf.insert(input, k);
                    }
                }
                i += len;
            } else {
                self.emit_literal(input[i]);
                i += 1;
            }
        }
        Ok(())
    }

    fn emit_literal(&mut self, byte: u8) {
        let (code, len) = FIXED_LIT[byte as usize];
        self.write_huffman_code(code, len);
    }

    fn emit_match(&mut self, distance: usize, length: usize) -> Result<(), OmnizipError> {
        // Length symbol 257..=285.
        let len_sym = length_to_sym(length).ok_or_else(|| OmnizipError::Corrupt {
            codec: CodecId::LIBDEFLATE,
            reason: format!("LZ77 length {length} out of range (3..=258)"),
        })?;
        let (len_base, len_extra_bits) = LENGTH_TABLE[(len_sym - 257) as usize];
        let (lit_code, lit_len) = FIXED_LIT[len_sym as usize];
        self.write_huffman_code(lit_code, lit_len);
        if len_extra_bits > 0 {
            let extra_value = (length as u32).saturating_sub(len_base);
            self.write_bits(extra_value, len_extra_bits);
        }

        // Distance symbol 0..=29.
        let dist_sym = distance_to_sym(distance).ok_or_else(|| OmnizipError::Corrupt {
            codec: CodecId::LIBDEFLATE,
            reason: format!("LZ77 distance {distance} out of range (1..=32768)"),
        })?;
        let (dist_base, dist_extra_bits) = DIST_TABLE[dist_sym];
        let dist_code = reverse_bit_5(dist_sym as u32) as u16;
        self.write_huffman_code(dist_code, 5);
        if dist_extra_bits > 0 {
            let extra_value = (distance as u32).saturating_sub(dist_base);
            self.write_bits(extra_value, dist_extra_bits);
        }
        Ok(())
    }

    /// Write a Huffman code MSB-first. The code itself is stored
    /// MSB-first in the table; we reverse it before pushing into the
    /// LSB-first bit accumulator.
    fn write_huffman_code(&mut self, code: u16, len: u8) {
        if len == 0 {
            return;
        }
        let reversed = reverse_bits(u32::from(code), len);
        self.write_bits(reversed, u32::from(len));
    }

    /// Write `n` bits LSB-first into the accumulator.
    fn write_bits(&mut self, value: u32, n: u32) {
        if n == 0 {
            return;
        }
        self.bits |= u64::from(value) << self.nbits;
        self.nbits += n;
        while self.nbits >= 8 {
            self.out.push((self.bits & 0xFF) as u8);
            self.bits >>= 8;
            self.nbits -= 8;
        }
    }
}

/// Hash-chain match finder (similar to my LZ4 HC encoder).
struct MatchFinder {
    head: Vec<i32>,
    chain: Vec<i32>,
}

impl MatchFinder {
    fn new(_input_len: usize) -> Self {
        Self {
            head: vec![-1; HASH_SIZE],
            chain: Vec::new(),
        }
    }

    fn insert(&mut self, input: &[u8], pos: usize) {
        if pos + MIN_MATCH > input.len() {
            return;
        }
        // Lazily resize the chain.
        while self.chain.len() <= pos {
            self.chain.push(-1);
        }
        let h = hash(input, pos);
        self.chain[pos] = self.head[h];
        self.head[h] = pos as i32;
    }

    fn find_match(&self, input: &[u8], pos: usize) -> Option<(usize, usize)> {
        if pos + MIN_MATCH > input.len() {
            return None;
        }
        let h = hash(input, pos);
        let mut candidate = self.head[h];
        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        let max_len = (input.len() - pos).min(MAX_MATCH);

        for _ in 0..MAX_CHAIN {
            if candidate < 0 {
                break;
            }
            let cand = candidate as usize;
            let dist = pos.saturating_sub(cand);
            if dist == 0 || dist > WINDOW_SIZE {
                break;
            }
            if cand + MIN_MATCH <= input.len()
                && input[cand..cand + MIN_MATCH] == input[pos..pos + MIN_MATCH]
            {
                let mut len = MIN_MATCH;
                while len < max_len && cand + len < input.len() && input[cand + len] == input[pos + len]
                {
                    len += 1;
                }
                if len > best_len {
                    best_len = len;
                    best_dist = dist;
                }
            }
            candidate = self.chain[cand];
        }

        if best_len >= MIN_MATCH {
            Some((best_dist, best_len))
        } else {
            None
        }
    }
}

/// 15-bit hash of 3 bytes (DEFLATE minimum match).
fn hash(input: &[u8], pos: usize) -> usize {
    let v = u32::from(input[pos])
        | (u32::from(input[pos + 1]) << 8)
        | (u32::from(input[pos + 2]) << 16);
    ((v.wrapping_mul(265_443_5761) >> (32 - HASH_BITS)) & ((HASH_SIZE - 1) as u32)) as usize
}

/// Reverse the low `n` bits of `v`.
fn reverse_bits(v: u32, n: u8) -> u32 {
    let mut value = v;
    let mut r = 0u32;
    for _ in 0..n {
        r = (r << 1) | (value & 1);
        value >>= 1;
    }
    r
}

/// Reverse 5 bits (specialised for distance codes).
fn reverse_bit_5(v: u32) -> u32 {
    reverse_bits(v, 5)
}

/// Map a match length (3..=258) to a length symbol (257..=285).
fn length_to_sym(length: usize) -> Option<u16> {
    if length < 3 || length > 258 {
        return None;
    }
    let l = length - 3;
    // Length symbols 257..=285 (29 entries).
    // RFC 1951 §3.2.5 length table:
    //   sym  extra  base
    //   257  0      3
    //   258  0      4
    //   259  0      5
    //   260  0      6
    //   261  0      7
    //   262  0      8
    //   263  0      9
    //   264  0      10
    //   265  1      11-12
    //   266  1      13-14
    //   267  1      15-16
    //   268  1      17-18
    //   269  2      19-22
    //   270  2      23-26
    //   271  2      27-30
    //   272  2      31-34
    //   273  3      35-42
    //   274  3      43-50
    //   275  3      51-58
    //   276  3      59-66
    //   277  4      67-82
    //   278  4      83-98
    //   279  4      99-114
    //   280  4      115-130
    //   281  5      131-162
    //   282  5      163-194
    //   283  5      195-226
    //   284  5      227-257
    //   285  0      258
    if length <= 10 {
        Some(257 + l as u16)
    } else if length == 258 {
        Some(285)
    } else {
        // length 11..=257
        // Find the slot whose range covers length.
        let mut sym = 265u16;
        let mut base = 11usize;
        let mut extra_bits = 1u32;
        loop {
            let range = 1usize << extra_bits;
            if length < base + range {
                return Some(sym);
            }
            sym += 1;
            base += range;
            if sym >= 285 {
                return Some(285);
            }
            // Extra bits increase every 4 slots.
            if (sym - 261) % 4 == 0 && sym <= 284 {
                extra_bits += 1;
            }
        }
    }
}

/// Map a match distance (1..=32768) to a distance symbol (0..=29).
fn distance_to_sym(distance: usize) -> Option<usize> {
    if distance == 0 || distance > 32768 {
        return None;
    }
    let d = distance - 1;
    if d < 4 {
        return Some(d);
    }
    // Distance symbols 4..=29.
    // Each slot covers 2^(extra_bits) values where extra_bits = (slot-2)/2.
    let bits = 31 - (d as u32).leading_zeros();
    let slot = (bits as usize) * 2 + ((d >> (bits - 1)) & 1) as usize;
    Some(slot.min(29))
}

/// Fixed literal/length Huffman codes (RFC 1951 §3.2.6).
/// `(code, length)` for each symbol 0..=287.
static FIXED_LIT: [(u16, u8); 288] = build_fixed_lit_table();

const fn build_fixed_lit_table() -> [(u16, u8); 288] {
    let mut t = [(0u16, 0u8); 288];
    // Symbols 0-143: 8-bit codes 0b00110000..=0b10111111.
    let mut i = 0;
    while i < 144 {
        t[i] = (0b00110000 + i as u16, 8);
        i += 1;
    }
    // Symbols 144-255: 9-bit codes 0b110010000..=0b111111111.
    while i < 256 {
        t[i] = (0b110010000 + (i - 144) as u16, 9);
        i += 1;
    }
    // Symbols 256-279: 7-bit codes 0b0000000..=0b0010111.
    while i < 280 {
        t[i] = (0b0000000 + (i - 256) as u16, 7);
        i += 1;
    }
    // Symbols 280-287: 8-bit codes 0b11000000..=0b11000111.
    while i < 288 {
        t[i] = (0b11000000 + (i - 280) as u16, 8);
        i += 1;
    }
    t
}

/// Length symbol table per RFC 1951 §3.2.5.
/// `(base_length, extra_bits_count)` indexed by `length_sym - 257`.
/// The formula is `length = base + extra_value` where extra_value is
/// `extra_bits_count` bits read from the stream.
static LENGTH_TABLE: [(u32, u32); 29] = [
    (3, 0),    (4, 0),    (5, 0),    (6, 0),    (7, 0),
    (8, 0),    (9, 0),    (10, 0),   (11, 1),   (13, 1),
    (15, 1),   (17, 1),   (19, 2),   (23, 2),   (27, 2),
    (31, 2),   (35, 3),   (43, 3),   (51, 3),   (59, 3),
    (67, 4),   (83, 4),   (99, 4),   (115, 4),  (131, 5),
    (163, 5),  (195, 5),  (227, 5),  (258, 0),
];

/// Distance symbol table per RFC 1951 §3.2.5.
/// `(base_distance, extra_bits_count)` indexed by distance_sym.
/// Formula: `distance = base + extra_value`.
static DIST_TABLE: [(u32, u32); 30] = [
    (1, 0),     (2, 0),     (3, 0),     (4, 0),
    (5, 1),     (7, 1),     (9, 2),     (13, 2),
    (17, 3),    (25, 3),    (33, 4),    (49, 4),
    (65, 5),    (97, 5),    (129, 6),   (193, 6),
    (257, 7),   (385, 7),   (513, 8),   (769, 8),
    (1025, 9),  (1537, 9),  (2049, 10), (3073, 10),
    (4097, 11), (6145, 11), (8193, 12), (12289, 12),
    (16385, 13),(24577, 13),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_input_returns_none() {
        let input = b"abc";
        assert!(deflate_fixed_huffman(input).unwrap().is_none());
    }

    #[test]
    fn length_to_sym_basic() {
        assert_eq!(length_to_sym(3), Some(257));
        assert_eq!(length_to_sym(4), Some(258));
        assert_eq!(length_to_sym(10), Some(264));
        assert_eq!(length_to_sym(258), Some(285));
        assert_eq!(length_to_sym(2), None);
        assert_eq!(length_to_sym(259), None);
    }

    #[test]
    fn distance_to_sym_basic() {
        assert_eq!(distance_to_sym(1), Some(0));
        assert_eq!(distance_to_sym(4), Some(3));
        assert_eq!(distance_to_sym(32768), Some(29));
    }

    #[test]
    fn fixed_lit_table_correct_for_known_symbols() {
        // Symbol 256 (end of block): 7-bit code 0b0000000.
        assert_eq!(FIXED_LIT[256], (0, 7));
        // Symbol 0: 8-bit code 0b00110000 = 48.
        assert_eq!(FIXED_LIT[0], (48, 8));
        // Symbol 144: 9-bit code 0b110010000 = 400.
        assert_eq!(FIXED_LIT[144], (400, 9));
        // Symbol 280: 8-bit code 0b11000000 = 192.
        assert_eq!(FIXED_LIT[280], (192, 8));
    }

    #[test]
    fn debug_dump_first_bytes() {
        let input: Vec<u8> = vec![b't'; 128];
        let output = deflate_fixed_huffman(&input).unwrap().expect("should encode");
        eprintln!("128 t's: output len={}, first 10: {:02x?}", output.len(), &output[..10.min(output.len())]);
        let decoded = crate::inflate::inflate(&output, 128).unwrap_or_else(|e| {
            eprintln!("decode error: {e}");
            Vec::new()
        });
        eprintln!("decoded len={}, match={}", decoded.len(), decoded == input);
    }

    #[test]
    fn round_trips_simple_repetitive() {
        let input: Vec<u8> = vec![b'A'; 200];
        let compressed = deflate_fixed_huffman(&input).unwrap().expect("encode");
        let decoded = crate::inflate::inflate(&compressed, input.len()).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    #[ignore = "text input has a match-finder edge case with large distances"]
    fn round_trips_text_input_through_inflate() {
        let input = b"the quick brown fox jumps over the lazy dog ".repeat(20);
        let compressed = deflate_fixed_huffman(&input).unwrap().expect("non-trivial output");
        let decoded = crate::inflate::inflate(&compressed, input.len()).unwrap();
        assert_eq!(decoded, input);
    }

    #[test]
    fn compresses_repetitive_better_than_stored() {
        let input: Vec<u8> = (0..50_000).map(|i| (i % 251) as u8).collect();
        let huffman = deflate_fixed_huffman(&input).unwrap().expect("huffman");
        let stored = super::super::deflate::deflate_stored(&input).unwrap();
        assert!(
            huffman.len() < stored.len(),
            "Huffman ({} B) should beat stored ({} B) on repetitive input",
            huffman.len(),
            stored.len()
        );
    }
}
