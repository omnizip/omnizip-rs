//! Deserialize and expand a GLZA-compressed stream.
//!
//! Parses the wire format produced by [`crate::encode`] and expands the
//! grammar back to the original bytes.
//!
//! ## Cycle safety
//!
//! The grammar builder enforces an append-only invariant (a rule may only
//! reference rules with strictly smaller ids), so cycles are impossible by
//! construction. As defence-in-depth, the decoder also tracks an expansion
//! depth budget and aborts if it exceeds a sane bound — this protects
//! against a malicious or corrupt payload.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use crate::encode::MAGIC;
use crate::grammar::Symbol;
use omnizip_codecs::{CodecId, OmnizipError};

/// GLZA codec id, used for error attribution.
pub const GLZA_CODEC_ID: CodecId = CodecId::new(0x000D);

/// Marker byte introducing either a rule reference or a literal escape.
const MARKER: u8 = 0xFF;

/// Maximum recursion depth when expanding rules. Generous bound for any
/// legitimate grammar; if exceeded we treat the payload as corrupt.
const MAX_DEPTH: u32 = 10_000;

/// Parsed grammar: the uncompressed size from the header, the start rule,
/// and the rule definitions.
pub type ParsedGrammar = (u32, Vec<Symbol>, Vec<Vec<Symbol>>);

/// Parse the GLZA wire format.
///
/// Returns `(uncompressed_size, start_rule, rules)`.
pub fn parse(compressed: &[u8]) -> Result<ParsedGrammar, OmnizipError> {
    if compressed.len() < 11 {
        return Err(OmnizipError::Corrupt {
            codec: GLZA_CODEC_ID,
            reason: format!("payload too short ({} bytes, need >= 11)", compressed.len()),
        });
    }
    if &compressed[..5] != MAGIC {
        return Err(OmnizipError::Corrupt {
            codec: GLZA_CODEC_ID,
            reason: format!("bad magic: {:02x?}", &compressed[..5]),
        });
    }

    let uncompressed_size = u32::from_le_bytes(compressed[5..9].try_into().unwrap());
    let rule_count = u16::from_le_bytes(compressed[9..11].try_into().unwrap()) as usize;

    let mut cursor = 11usize;
    let mut start_rule: Vec<Symbol> = Vec::new();
    let mut rules: Vec<Vec<Symbol>> = Vec::with_capacity(rule_count);

    // Read start rule + rule_count rule definitions.
    let total_rules = rule_count + 1;
    for i in 0..total_rules {
        let (syms, consumed) =
            read_rule(&compressed[cursor..]).ok_or_else(|| OmnizipError::Corrupt {
                codec: GLZA_CODEC_ID,
                reason: format!("truncated rule body at rule index {i}"),
            })?;
        cursor += consumed;
        if i == 0 {
            start_rule = syms;
        } else {
            // Verify every rule reference points to a rule that has already
            // been parsed OR will be parsed later — but more importantly
            // verify the append-only invariant (ref must be < current rule
            // index in the rules vec).
            let rule_idx = i - 1;
            for s in &syms {
                if let Symbol::Rule(ref_id) = s {
                    if (*ref_id as usize) >= rule_idx {
                        return Err(OmnizipError::Corrupt {
                            codec: GLZA_CODEC_ID,
                            reason: format!(
                                "rule {rule_idx} references rule {ref_id} which is not strictly smaller — cyclic grammar"
                            ),
                        });
                    }
                }
            }
            rules.push(syms);
        }
    }

    Ok((uncompressed_size, start_rule, rules))
}

/// Read one rule body: a varint length prefix, then that many symbols.
/// Returns `(symbols, bytes_consumed)` or `None` if truncated.
fn read_rule(data: &[u8]) -> Option<(Vec<Symbol>, usize)> {
    let (count, mut pos) = read_varint(data)?;
    let count = count as usize;
    let mut syms: Vec<Symbol> = Vec::with_capacity(count);
    for _ in 0..count {
        if pos >= data.len() {
            return None;
        }
        let b = data[pos];
        if b == MARKER {
            // Either rule ref or literal escape. Peek next byte.
            pos += 1;
            if pos >= data.len() {
                return None;
            }
            let (val, consumed) = read_varint(&data[pos..])?;
            pos += consumed;
            if val == 0 {
                // MARKER followed by varint 0 = literal escape for byte 0xFF.
                syms.push(Symbol::Byte(MARKER));
            } else {
                // val = n + 1, so n = val - 1.
                let n = val - 1;
                if n > u16::MAX as u64 {
                    return None;
                }
                syms.push(Symbol::Rule(n as u16));
            }
        } else {
            syms.push(Symbol::Byte(b));
            pos += 1;
        }
    }
    Some((syms, pos))
}

/// Read a varint (LEB128 unsigned). Returns `(value, bytes_consumed)`.
fn read_varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    for (i, &b) in data.iter().enumerate() {
        result |= u64::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
        if shift >= 64 {
            return None; // varint too large
        }
    }
    None // truncated
}

/// Expand a parsed grammar to the original byte sequence.
///
/// `uncompressed_size` is checked against the actual expanded length and
/// the call returns [`OmnizipError::LengthMismatch`] on divergence.
pub fn expand(
    uncompressed_size: u32,
    start_rule: &[Symbol],
    rules: &[Vec<Symbol>],
) -> Result<Vec<u8>, OmnizipError> {
    let mut out: Vec<u8> = Vec::new();
    for &s in start_rule {
        expand_symbol(s, rules, &mut out, 0)?;
    }
    if out.len() as u32 != uncompressed_size {
        return Err(OmnizipError::LengthMismatch {
            codec: GLZA_CODEC_ID,
            expected: uncompressed_size,
            actual: out.len(),
        });
    }
    Ok(out)
}

fn expand_symbol(
    s: Symbol,
    rules: &[Vec<Symbol>],
    out: &mut Vec<u8>,
    depth: u32,
) -> Result<(), OmnizipError> {
    if depth > MAX_DEPTH {
        return Err(OmnizipError::Corrupt {
            codec: GLZA_CODEC_ID,
            reason: format!("expansion depth exceeded {MAX_DEPTH} — cyclic grammar suspected"),
        });
    }
    match s {
        Symbol::Byte(b) => out.push(b),
        Symbol::Rule(n) => {
            let idx = n as usize;
            let body = rules.get(idx).ok_or_else(|| OmnizipError::Corrupt {
                codec: GLZA_CODEC_ID,
                reason: format!(
                    "rule reference {n} out of range (have {} rules)",
                    rules.len()
                ),
            })?;
            for &inner in body {
                expand_symbol(inner, rules, out, depth + 1)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_magic() {
        let bad = b"XXXX\0\0\0\0\0\0\0";
        assert!(parse(bad).is_err());
    }

    #[test]
    fn rejects_too_short() {
        assert!(parse(b"GLZ").is_err());
    }

    #[test]
    fn empty_payload_parses() {
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        // start rule: varint 0 (no symbols)
        buf.push(0);
        let (sz, start, rules) = parse(&buf).expect("parse");
        assert_eq!(sz, 0);
        assert!(start.is_empty());
        assert!(rules.is_empty());
    }

    #[test]
    fn rejects_cyclic_grammar() {
        // Hand-craft a payload where rule 0 references itself.
        // rule_count = 1, start_rule is empty, rule_0 has one Symbol::Rule(0).
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        // start rule: varint 0
        buf.push(0);
        // rule 0: varint 1, then one symbol = Rule(0) = MARKER, varint(0+1)=1
        buf.push(1);
        buf.push(MARKER);
        buf.push(1);
        let err = parse(&buf);
        assert!(err.is_err(), "cyclic grammar must be rejected");
    }

    #[test]
    fn expand_round_trips_simple() {
        // Grammar: start = [Rule(0)], rule_0 = [Byte(0x41), Byte(0x42)]
        // Output: "AB"
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        // start rule: 1 symbol = Rule(0)
        buf.push(1);
        buf.push(MARKER);
        buf.push(1);
        // rule 0: 2 symbols = byte 0x41, byte 0x42
        buf.push(2);
        buf.push(0x41);
        buf.push(0x42);
        let (sz, start, rules) = parse(&buf).expect("parse");
        let out = expand(sz, &start, &rules).expect("expand");
        assert_eq!(out, b"AB");
    }
}
