//! Serialize a [`Grammar`] to the GLZA wire format.
//!
//! ## Wire format
//!
//! ```text
//! +---------------------+  5 bytes: magic b"GLZA\0"
//! | magic               |
//! +---------------------+  1 byte:  container version (1 = raw, 2 = Huffman)
//! | version             |
//! +---------------------+  4 bytes LE: uncompressed_size
//! | uncompressed_size   |
//! +---------------------+  2 bytes LE: rule_count
//! | rule_count          |
//! +---------------------+  variable: body (depends on version)
//! | body                |
//! +---------------------+
//! ```
//!
//! ### Version 1 body (raw varints)
//!
//! Each rule is preceded by a varint symbol count, then a sequence of
//! symbols. A symbol is encoded as:
//!
//! - If the byte is `0xFF`: emit `0xFF 0x00` (literal escape).
//! - `Symbol::Byte(b)` where `b != 0xFF`: emit `[b]` (1 byte).
//! - `Symbol::Rule(n)`: emit `0xFF` followed by varint `n + 1`.
//!
//! ### Version 2 body (Huffman-coded)
//!
//! ```text
//! huff_alphabet_size:u16 LE
//! huff_code_lengths: [u8; alphabet_size]   (canonical lengths, 0 = unused)
//! body_byte_len:u32 LE
//! raw_counts:  varint symbol count for each rule (start rule, then defs)
//! bit_packed:  Huffman codes for each symbol, MSB-first, concatenated
//! ```

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use crate::entropy::{
    self, alphabet_size, canonical_codes, symbol_frequencies, symbol_to_index, BitWriter,
};
use crate::grammar::{Grammar, Symbol};

/// Magic header for the GLZA wire format.
pub const MAGIC: &[u8; 5] = b"GLZA\0";

/// Container version byte. `1` = raw Phase 1, `2` = Huffman-coded Phase 2.
pub const VERSION_RAW: u8 = 1;
pub const VERSION_HUFFMAN: u8 = 2;

/// Byte value used as the rule-reference marker (and as the literal-escape
/// prefix when followed by another 0xFF).
const MARKER: u8 = 0xFF;

/// Encode `grammar` to bytes using the Phase 1 (raw varint) wire format,
/// recording `uncompressed_size` (the length of the original input) in the
/// header.
#[must_use]
#[allow(dead_code)]
pub fn encode(grammar: &Grammar, uncompressed_size: u32) -> Vec<u8> {
    encode_v1(grammar, uncompressed_size)
}

/// Encode with an explicit container version.
///
/// `version == 1` produces the Phase 1 raw stream. `version == 2` produces
/// the Phase 2 Huffman-coded stream. Other values fall back to v1.
#[must_use]
pub fn encode_with_version(grammar: &Grammar, uncompressed_size: u32, version: u8) -> Vec<u8> {
    if version == VERSION_HUFFMAN {
        encode_v2(grammar, uncompressed_size)
    } else {
        encode_v1(grammar, uncompressed_size)
    }
}

/// Phase 1 raw varint encoding. Wire format:
///
/// ```text
/// MAGIC (5) | version:u8=1 | uncompressed_size:u32 LE |
/// rule_count:u16 LE | start rule + each rule body
/// ```
#[must_use]
pub fn encode_v1(grammar: &Grammar, uncompressed_size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + grammar.symbol_count() * 2);
    out.extend_from_slice(MAGIC);
    out.push(VERSION_RAW);
    out.extend_from_slice(&uncompressed_size.to_le_bytes());
    let rule_count = u16::try_from(grammar.rules.len()).unwrap_or(u16::MAX);
    out.extend_from_slice(&rule_count.to_le_bytes());

    // Start rule.
    encode_rule(&mut out, &grammar.start_rule);
    // Each rule definition.
    for rule in &grammar.rules {
        encode_rule(&mut out, rule);
    }
    out
}

/// Phase 2 Huffman-coded encoding. Wire format:
///
/// ```text
/// MAGIC (5) | version:u8=2 | uncompressed_size:u32 LE |
/// rule_count:u16 LE |
/// huff_alphabet_size:u16 LE |
/// huff_code_lengths: [u8; alphabet_size] |
/// body_byte_len:u32 LE |
/// bit-packed body:
///   for each rule (start rule first, then each definition):
///     varint symbol_count (raw)
///     for each symbol: its canonical Huffman code, MSB-first
/// ```
#[must_use]
pub fn encode_v2(grammar: &Grammar, uncompressed_size: u32) -> Vec<u8> {
    let alphabet = alphabet_size(grammar);
    // Cap alphabet at u16::MAX; if the grammar has too many rules we fall
    // back to v1 (which has the same rule cap behaviour).
    if alphabet > u16::MAX as u32 {
        return encode_v1(grammar, uncompressed_size);
    }

    let freq = symbol_frequencies(grammar);
    let lengths = entropy::code_lengths(&freq);
    let codes = canonical_codes(&lengths);

    // Empty grammar: emit a minimal v2 header. The decoder reconstructs an
    // empty start rule from rule_count + an empty bit-packed region.
    let alphabet_u16 = u16::try_from(alphabet).unwrap_or(u16::MAX);

    // Build the bit-packed body. We interleave varint symbol counts (raw)
    // with Huffman-coded symbols.
    let mut writer = BitWriter::new();
    // Write start rule's symbols, then each rule's symbols. Symbol counts
    // go through a separate raw byte buffer so they remain byte-aligned.
    let mut raw_counts: Vec<u8> = Vec::new();
    let mut emit_rule = |rule: &[Symbol]| {
        write_varint(&mut raw_counts, rule.len() as u64);
        for s in rule {
            let idx = symbol_to_index(*s) as usize;
            let (code, len) = codes[idx];
            writer.write_bits(code, len);
        }
    };
    emit_rule(&grammar.start_rule);
    for rule in &grammar.rules {
        emit_rule(rule);
    }
    let bit_packed = writer.flush();

    let body_byte_len = u32::try_from(raw_counts.len() + bit_packed.len()).unwrap_or(u32::MAX);

    let mut out = Vec::with_capacity(
        5 + 1 + 4 + 2 + 2 + lengths.len() + 4 + raw_counts.len() + bit_packed.len(),
    );
    out.extend_from_slice(MAGIC);
    out.push(VERSION_HUFFMAN);
    out.extend_from_slice(&uncompressed_size.to_le_bytes());
    let rule_count = u16::try_from(grammar.rules.len()).unwrap_or(u16::MAX);
    out.extend_from_slice(&rule_count.to_le_bytes());
    out.extend_from_slice(&alphabet_u16.to_le_bytes());
    out.extend_from_slice(&lengths);
    out.extend_from_slice(&body_byte_len.to_le_bytes());
    out.extend_from_slice(&raw_counts);
    out.extend_from_slice(&bit_packed);
    out
}

fn encode_rule(out: &mut Vec<u8>, rule: &[Symbol]) {
    write_varint(out, rule.len() as u64);
    for s in rule {
        match s {
            Symbol::Byte(b) => {
                if *b == MARKER {
                    // Literal escape: MARKER followed by varint 0 (a single
                    // 0x00 byte). This is distinct from rule refs, which
                    // encode n+1 (always >= 1).
                    out.push(MARKER);
                    out.push(0x00);
                } else {
                    out.push(*b);
                }
            }
            Symbol::Rule(n) => {
                out.push(MARKER);
                // Encode n+1 as varint (always >= 1, so never collides with
                // the literal-escape sequence MARKER 0x00).
                write_varint(out, u64::from(*n) + 1);
            }
        }
    }
}

/// Write a varint (LEB128 unsigned) to `out`.
fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_round_trips() {
        fn rt(v: u64) {
            let mut buf = Vec::new();
            write_varint(&mut buf, v);
            assert!(buf.len() <= 10);
            // Manual decode.
            let mut got = 0u64;
            let mut shift = 0;
            for (i, &b) in buf.iter().enumerate() {
                let low = u64::from(b & 0x7F);
                got |= low << shift;
                if b & 0x80 == 0 {
                    assert_eq!(i + 1, buf.len());
                    break;
                }
                shift += 7;
            }
            assert_eq!(got, v, "varint mismatch for {v}");
        }
        for v in [
            0,
            1,
            127,
            128,
            255,
            256,
            16384,
            1_000_000,
            u32::MAX as u64,
            u64::MAX,
        ] {
            rt(v);
        }
    }

    #[test]
    fn empty_grammar_header() {
        let g = Grammar {
            start_rule: Vec::new(),
            rules: Vec::new(),
        };
        let out = encode(&g, 0);
        assert_eq!(&out[..5], MAGIC);
        assert_eq!(out[5], VERSION_RAW);
        assert_eq!(u32::from_le_bytes(out[6..10].try_into().unwrap()), 0);
        assert_eq!(u16::from_le_bytes(out[10..12].try_into().unwrap()), 0);
    }

    #[test]
    fn literal_escape_byte() {
        // A start rule containing 0xFF must encode it as MARKER 0x00.
        let g = Grammar {
            start_rule: vec![Symbol::Byte(0xFF), Symbol::Byte(0x42)],
            rules: Vec::new(),
        };
        let out = encode(&g, 2);
        // header (12 = 5 magic + 1 version + 4 size + 2 rule_count)
        // + varint(count=2)=1 byte + MARKER 0x00 0x42
        assert_eq!(out.len(), 12 + 1 + 3);
        assert_eq!(&out[13..16], &[0xFF, 0x00, 0x42]);
    }
}
