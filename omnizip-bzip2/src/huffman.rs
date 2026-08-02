//! Canonical Huffman coding — the final entropy stage of `BZip2`.
//!
//! Port of `omnizip/lib/omnizip/algorithms/bzip2/huffman.rb` plus the
//! canonical-code generation in `encoder.rb` / `decoder.rb`.
//!
//! The wire format stores code *lengths* (not the codes themselves); encoder
//! and decoder independently derive the same canonical codes from those
//! lengths, guaranteeing determinism.

use std::collections::BTreeMap;

/// A code length table: `lengths[symbol] = code_length_in_bits`.
///
/// Symbols that do not appear are absent from the map (treated as length 0).
pub type CodeLengths = BTreeMap<u8, u8>;

/// A frequency table: `symbol -> occurrence count`. Counts are `u64` so they
/// cannot overflow on realistic block sizes.
pub type FreqTable = BTreeMap<u8, u64>;

/// A canonical code assignment: `symbol -> (code_value, code_length)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CanonicalCode {
    pub value: u32,
    pub length: u8,
}

/// Huffman tree node used while building code lengths.
struct Node {
    freq: u64,
    left: Option<usize>,
    right: Option<usize>,
    symbol: Option<u8>,
}

/// Build a Huffman tree from symbol frequencies and return per-symbol code
/// lengths.
///
/// Returns an empty map for empty input. A single unique symbol gets length 1.
#[must_use]
pub fn build_code_lengths(freqs: &FreqTable) -> CodeLengths {
    if freqs.is_empty() {
        return CodeLengths::new();
    }

    // Special case: one symbol. Assign it a 1-bit code so decoding works.
    if freqs.len() == 1 {
        let mut out = CodeLengths::new();
        for &symbol in freqs.keys() {
            out.insert(symbol, 1);
        }
        return out;
    }

    let mut nodes: Vec<Node> = Vec::new();
    for (&symbol, &freq) in freqs {
        nodes.push(Node {
            freq,
            left: None,
            right: None,
            symbol: Some(symbol),
        });
    }

    // Active node indices, sorted ascending by (frequency, index) for
    // deterministic tie-breaking. We repeatedly merge the two smallest.
    let mut active: Vec<usize> = (0..nodes.len()).collect();
    active.sort_by_key(|&i| (nodes[i].freq, i));

    while active.len() > 1 {
        let a = active.remove(0);
        let b = active.remove(0);
        let combined = nodes[a].freq + nodes[b].freq;
        nodes.push(Node {
            freq: combined,
            left: Some(a),
            right: Some(b),
            symbol: None,
        });
        let new_idx = nodes.len() - 1;
        let pos = active.partition_point(|&i| (nodes[i].freq, i) < (combined, new_idx));
        active.insert(pos, new_idx);
    }

    let mut lengths = CodeLengths::new();
    walk(&nodes, active[0], 0, &mut lengths);
    lengths
}

fn walk(nodes: &[Node], idx: usize, depth: u8, out: &mut CodeLengths) {
    let node = &nodes[idx];
    if let Some(sym) = node.symbol {
        out.insert(sym, depth.max(1));
        return;
    }
    if let Some(l) = node.left {
        walk(nodes, l, depth + 1, out);
    }
    if let Some(r) = node.right {
        walk(nodes, r, depth + 1, out);
    }
}

/// Derive canonical codes from code lengths.
///
/// Sort symbols by `(length, symbol)`; assign sequential code values,
/// left-shifting when the length increases. This matches the algorithm in
/// `encoder.rb#generate_canonical_codes` and `decoder.rb#rebuild_huffman_tree`.
#[must_use]
pub fn canonical_codes(lengths: &CodeLengths) -> Vec<(u8, CanonicalCode)> {
    // BTreeMap iterates in symbol order; sort by length, keeping symbol as
    // tiebreaker (stable sort preserves the symbol-ascending input order).
    let mut sorted: Vec<(u8, u8)> = lengths.iter().map(|(&s, &l)| (s, l)).collect();
    sorted.sort_by_key(|&(_s, l)| l);

    let mut out = Vec::with_capacity(sorted.len());
    let mut code_value: u32 = 0;
    let mut prev_length: u8 = 0;
    for (symbol, length) in sorted {
        if length == 0 {
            continue;
        }
        let shift = length - prev_length;
        code_value <<= shift;
        out.push((
            symbol,
            CanonicalCode {
                value: code_value,
                length,
            },
        ));
        code_value += 1;
        prev_length = length;
    }
    out
}

/// Huffman-encode `data` using the provided code lengths.
///
/// Returns `(packed_bytes, padding_bits)` where `padding_bits` is the number
/// of zero bits added to round up to a whole byte.
#[must_use]
pub fn huffman_encode(data: &[u8], lengths: &CodeLengths) -> (Vec<u8>, u8) {
    if data.is_empty() {
        return (Vec::new(), 0);
    }
    let codes = canonical_codes(lengths);
    let mut lookup = [CanonicalCode {
        value: 0,
        length: 0,
    }; 256];
    for (sym, code) in codes {
        lookup[usize::from(sym)] = code;
    }

    let mut out: Vec<u8> = Vec::new();
    let mut current_byte: u32 = 0;
    let mut bits_in_byte: u8 = 0;

    for &sym in data {
        let code = lookup[usize::from(sym)];
        let len = code.length.max(1);
        // Push MSB-first.
        for bit_idx in (0..len).rev() {
            let bit = (code.value >> bit_idx) & 1;
            current_byte = (current_byte << 1) | bit;
            bits_in_byte += 1;
            if bits_in_byte == 8 {
                out.push(current_byte as u8);
                current_byte = 0;
                bits_in_byte = 0;
            }
        }
    }

    let padding = if bits_in_byte > 0 {
        current_byte <<= 8 - bits_in_byte;
        out.push(current_byte as u8);
        8 - bits_in_byte
    } else {
        0
    };

    (out, padding)
}

/// Huffman-decode `packed` bits back to symbols.
///
/// `lengths` is the same table the encoder used. `expected_len` is the number
/// of symbols to emit. `padding` is the number of trailing zero bits added by
/// the encoder.
///
/// # Errors
///
/// Returns an error message string if the bit stream is exhausted before
/// `expected_len` symbols are decoded, or if a prefix doesn't match any code.
pub fn huffman_decode(
    packed: &[u8],
    lengths: &CodeLengths,
    expected_len: usize,
    padding: u8,
) -> Result<Vec<u8>, String> {
    if expected_len == 0 {
        return Ok(Vec::new());
    }

    let codes = canonical_codes(lengths);
    // Build a (length, code_value) -> symbol lookup. Linear scan is fine for
    // a 256-symbol alphabet and keeps the code simple.
    let table: Vec<(u8, u32, u8)> = codes
        .iter()
        .map(|(sym, c)| (c.length, c.value, *sym))
        .collect();

    let mut out = Vec::with_capacity(expected_len);
    let mut acc: u32 = 0;
    let mut acc_len: u8 = 0;

    let total_bits = packed
        .len()
        .saturating_mul(8)
        .saturating_sub(usize::from(padding));
    let mut bit_consumed = 0usize;

    for &byte in packed {
        for bit_pos in (0..8).rev() {
            if bit_consumed >= total_bits {
                break;
            }
            bit_consumed += 1;
            let bit = u32::from((byte >> bit_pos) & 1);
            acc = (acc << 1) | bit;
            acc_len += 1;

            for &(len, value, sym) in &table {
                if len == acc_len && value == acc {
                    out.push(sym);
                    acc = 0;
                    acc_len = 0;
                    break;
                }
            }
            if out.len() == expected_len {
                return Ok(out);
            }
        }
        if out.len() >= expected_len {
            break;
        }
    }

    if out.len() == expected_len {
        Ok(out)
    } else {
        Err(format!(
            "Huffman decode exhausted at {} symbols (expected {expected_len})",
            out.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn freqs_from(data: &[u8]) -> FreqTable {
        let mut f = FreqTable::new();
        for &b in data {
            *f.entry(b).or_insert(0) += 1;
        }
        f
    }

    #[test]
    fn empty_round_trips() {
        let lengths = CodeLengths::new();
        let (enc, pad) = huffman_encode(b"", &lengths);
        assert!(enc.is_empty());
        let dec = huffman_decode(&enc, &lengths, 0, pad).unwrap();
        assert!(dec.is_empty());
    }

    #[test]
    fn single_symbol() {
        let data = b"aaaa";
        let f = freqs_from(data);
        let lengths = build_code_lengths(&f);
        let (enc, pad) = huffman_encode(data, &lengths);
        let dec = huffman_decode(&enc, &lengths, data.len(), pad).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trip_text() {
        let data = b"the quick brown fox";
        let f = freqs_from(data);
        let lengths = build_code_lengths(&f);
        let (enc, pad) = huffman_encode(data, &lengths);
        let dec = huffman_decode(&enc, &lengths, data.len(), pad).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trip_many_symbols() {
        let data: Vec<u8> = (0..200u32).map(|i| (i % 30) as u8).collect();
        let f = freqs_from(&data);
        let lengths = build_code_lengths(&f);
        let (enc, pad) = huffman_encode(&data, &lengths);
        let dec = huffman_decode(&enc, &lengths, data.len(), pad).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn canonical_codes_are_deterministic() {
        let mut f = FreqTable::new();
        f.insert(b'a', 5);
        f.insert(b'b', 3);
        f.insert(b'c', 1);
        let lengths = build_code_lengths(&f);
        let c1 = canonical_codes(&lengths);
        let c2 = canonical_codes(&lengths);
        assert_eq!(c1, c2);
    }
}
