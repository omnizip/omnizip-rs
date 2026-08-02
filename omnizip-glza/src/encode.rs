//! Serialize a [`Grammar`] to the GLZA wire format.
//!
//! ## Wire format
//!
//! ```text
//! +---------------------+  5 bytes: magic b"GLZA\0"
//! | magic               |
//! +---------------------+  4 bytes LE: uncompressed_size
//! | uncompressed_size   |
//! +---------------------+  2 bytes LE: rule_count
//! | rule_count          |
//! +---------------------+  variable: start rule, then each rule definition
//! | body                |
//! +---------------------+
//! ```
//!
//! Each rule is preceded by a varint symbol count, then a sequence of
//! symbols. A symbol is encoded as:
//!
//! - `Symbol::Byte(b)` -> a single byte with the high bit clear
//!   (`0b0_xxxxxxxx`, value `0x00–0x7F`... actually we use the full byte
//!   range since rule refs use a distinct prefix). To disambiguate from
//!   rule refs, we encode rule refs with a 2-byte marker.
//!
//! Precise per-symbol encoding:
//!
//! - If the byte is `0xFF`: emit `0xFF 0xFF` (literal escape).
//! - `Symbol::Byte(b)` where `b != 0xFF`: emit `[b]` (1 byte).
//! - `Symbol::Rule(n)`: emit `0xFF` followed by varint `n + 1` (so that
//!   `0xFF 0x00` is unused; we always have n+1 >= 1).
//!
//! Each rule body is length-prefixed by a varint symbol count.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use crate::grammar::{Grammar, Symbol};

/// Magic header for the GLZA wire format.
pub const MAGIC: &[u8; 5] = b"GLZA\0";

/// Byte value used as the rule-reference marker (and as the literal-escape
/// prefix when followed by another 0xFF).
const MARKER: u8 = 0xFF;

/// Encode `grammar` to bytes, recording `uncompressed_size` (the length of
/// the original input) in the header.
#[must_use]
pub fn encode(grammar: &Grammar, uncompressed_size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + grammar.symbol_count() * 2);
    out.extend_from_slice(MAGIC);
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
        assert_eq!(u32::from_le_bytes(out[5..9].try_into().unwrap()), 0);
        assert_eq!(u16::from_le_bytes(out[9..11].try_into().unwrap()), 0);
    }

    #[test]
    fn literal_escape_byte() {
        // A start rule containing 0xFF must encode it as MARKER 0x00.
        let g = Grammar {
            start_rule: vec![Symbol::Byte(0xFF), Symbol::Byte(0x42)],
            rules: Vec::new(),
        };
        let out = encode(&g, 2);
        // header (11) + varint(count=2)=1 byte + MARKER 0x00 0x42
        assert_eq!(out.len(), 11 + 1 + 3);
        assert_eq!(&out[12..15], &[0xFF, 0x00, 0x42]);
    }
}
