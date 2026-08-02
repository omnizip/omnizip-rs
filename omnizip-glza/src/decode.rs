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

use crate::encode::{MAGIC, VERSION_HUFFMAN, VERSION_RAW};
use crate::entropy::{index_to_symbol, BitReader, HuffmanDecoder};
use crate::grammar::Symbol;
use omnizip_codecs::{CodecId, OmnizipError};

/// GLZA codec id, used for error attribution.

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
/// Returns `(uncompressed_size, start_rule, rules)`. Dispatches on the
/// container version byte: `1` = raw varints, `2` = Huffman-coded.
pub fn parse(compressed: &[u8]) -> Result<ParsedGrammar, OmnizipError> {
    if compressed.len() < 12 {
        return Err(OmnizipError::Corrupt {
            codec: CodecId::GLZA,
            reason: format!("payload too short ({} bytes, need >= 12)", compressed.len()),
        });
    }
    if &compressed[..5] != MAGIC {
        return Err(OmnizipError::Corrupt {
            codec: CodecId::GLZA,
            reason: format!("bad magic: {:02x?}", &compressed[..5]),
        });
    }

    let version = compressed[5];
    let uncompressed_size = u32::from_le_bytes(compressed[6..10].try_into().unwrap());
    let rule_count = u16::from_le_bytes(compressed[10..12].try_into().unwrap()) as usize;

    match version {
        VERSION_RAW => parse_v1_body(&compressed[12..], uncompressed_size, rule_count),
        VERSION_HUFFMAN => parse_v2_body(&compressed[12..], uncompressed_size, rule_count),
        other => Err(OmnizipError::Corrupt {
            codec: CodecId::GLZA,
            reason: format!("unsupported container version {other}"),
        }),
    }
}

/// Parse a Phase 1 raw-varint body (everything after the `rule_count` field).
fn parse_v1_body(
    body: &[u8],
    uncompressed_size: u32,
    rule_count: usize,
) -> Result<ParsedGrammar, OmnizipError> {
    let mut cursor = 0usize;
    let mut start_rule: Vec<Symbol> = Vec::new();
    let mut rules: Vec<Vec<Symbol>> = Vec::with_capacity(rule_count);

    let total_rules = rule_count + 1;
    for i in 0..total_rules {
        let (syms, consumed) = read_rule(&body[cursor..]).ok_or_else(|| OmnizipError::Corrupt {
            codec: CodecId::GLZA,
            reason: format!("truncated rule body at rule index {i}"),
        })?;
        cursor += consumed;
        if i == 0 {
            start_rule = syms;
        } else {
            let rule_idx = i - 1;
            for s in &syms {
                if let Symbol::Rule(ref_id) = s {
                    if (*ref_id as usize) >= rule_idx {
                        return Err(OmnizipError::Corrupt {
                            codec: CodecId::GLZA,
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

/// Parse a Phase 2 Huffman-coded body.
fn parse_v2_body(
    body: &[u8],
    uncompressed_size: u32,
    rule_count: usize,
) -> Result<ParsedGrammar, OmnizipError> {
    // huff_alphabet_size:u16 LE
    if body.len() < 2 {
        return Err(OmnizipError::Corrupt {
            codec: CodecId::GLZA,
            reason: "v2 body too short for alphabet size".to_string(),
        });
    }
    let alphabet_size = u16::from_le_bytes(body[..2].try_into().unwrap()) as usize;
    let mut cursor = 2usize;

    // Sanity: alphabet must be at least 256 and at most 256 + rule_count.
    // We allow alphabet >= 256 + rule_count exactly; smaller is corrupt.
    let expected_alphabet = 256 + rule_count;
    if alphabet_size < 256 || alphabet_size < expected_alphabet {
        return Err(OmnizipError::Corrupt {
            codec: CodecId::GLZA,
            reason: format!(
                "alphabet size {alphabet_size} smaller than expected {expected_alphabet}"
            ),
        });
    }

    // huff_code_lengths: [u8; alphabet_size]
    if body.len() < cursor + alphabet_size {
        return Err(OmnizipError::Corrupt {
            codec: CodecId::GLZA,
            reason: format!(
                "v2 body too short for code lengths (need {alphabet_size}, have {})",
                body.len() - cursor
            ),
        });
    }
    let lengths = &body[cursor..cursor + alphabet_size];
    cursor += alphabet_size;

    let decoder = HuffmanDecoder::from_lengths(lengths);

    // body_byte_len:u32 LE
    if body.len() < cursor + 4 {
        return Err(OmnizipError::Corrupt {
            codec: CodecId::GLZA,
            reason: "v2 body too short for body_byte_len".to_string(),
        });
    }
    let body_byte_len = u32::from_le_bytes(body[cursor..cursor + 4].try_into().unwrap()) as usize;
    cursor += 4;

    if body.len() < cursor + body_byte_len {
        return Err(OmnizipError::Corrupt {
            codec: CodecId::GLZA,
            reason: format!(
                "v2 body too short: declared {body_byte_len} bytes, have {}",
                body.len() - cursor
            ),
        });
    }
    let body_region = &body[cursor..cursor + body_byte_len];

    // Now we need to read: for each of (rule_count + 1) rules, a varint
    // symbol count (byte-aligned, from the front of body_region) and then
    // `count` Huffman-coded symbols (bit-packed, following the counts).
    //
    // Layout chosen by the encoder: all varint counts come first
    // (byte-aligned), then the bit-packed symbol stream follows.
    //
    // We read the counts first, accumulating how many bytes they consume,
    // then hand the remainder to the bit reader.
    let mut counts: Vec<usize> = Vec::with_capacity(rule_count + 1);
    let mut cpos = 0usize;
    for i in 0..=rule_count {
        let (c, consumed) =
            read_varint(&body_region[cpos..]).ok_or_else(|| OmnizipError::Corrupt {
                codec: CodecId::GLZA,
                reason: format!("truncated varint count at rule index {i}"),
            })?;
        cpos += consumed;
        counts.push(c as usize);
    }
    let bit_region = &body_region[cpos..];
    let mut reader = BitReader::new(bit_region);

    let mut start_rule: Vec<Symbol> = Vec::new();
    let mut rules: Vec<Vec<Symbol>> = Vec::with_capacity(rule_count);

    for (i, &count) in counts.iter().enumerate() {
        let mut syms: Vec<Symbol> = Vec::with_capacity(count);
        for _ in 0..count {
            let idx = decoder
                .decode(&mut reader)
                .ok_or_else(|| OmnizipError::Corrupt {
                    codec: CodecId::GLZA,
                    reason: "bit-packed symbol stream exhausted mid-code".to_string(),
                })?;
            syms.push(index_to_symbol(idx));
        }
        if i == 0 {
            start_rule = syms;
        } else {
            let rule_idx = i - 1;
            for s in &syms {
                if let Symbol::Rule(ref_id) = s {
                    if (*ref_id as usize) >= rule_idx {
                        return Err(OmnizipError::Corrupt {
                            codec: CodecId::GLZA,
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
            codec: CodecId::GLZA,
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
            codec: CodecId::GLZA,
            reason: format!("expansion depth exceeded {MAX_DEPTH} — cyclic grammar suspected"),
        });
    }
    match s {
        Symbol::Byte(b) => out.push(b),
        Symbol::Rule(n) => {
            let idx = n as usize;
            let body = rules.get(idx).ok_or_else(|| OmnizipError::Corrupt {
                codec: CodecId::GLZA,
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
        // 5 bytes magic-area "XXXX\0" + version + 4 size + 2 rule_count = 12
        let bad = b"XXXX\0\x01\0\0\0\0\0\0";
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
        buf.push(VERSION_RAW);
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
        buf.push(VERSION_RAW);
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
        buf.push(VERSION_RAW);
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
