//! Huffman coding for Deflate64.
//!
//! Direct port of `omnizip/lib/omnizip/algorithms/deflate64/huffman_coder.rb`.
//! Builds canonical Huffman trees from symbol frequencies and serialises the
//! resulting code table alongside the bitstream.
//!
//! # Determinism
//!
//! Tree construction uses a stable sort with deterministic tie-breaking so
//! that identical input always yields byte-identical output (required by
//! `LimniFS` content addressing).

#![allow(clippy::cast_possible_truncation)]

use crate::constants::MAX_MATCH_LENGTH;

/// Length-code table: `(base_length, extra_bits)` for codes 257..=285.
/// Matches RFC 1951 §3.2.5. A length `L` with code `C` satisfies
/// `L = base + extra`, where `extra` is the value carried in the
/// `extra_bits` bit field following the Huffman code.
const LENGTH_TABLE: [(u16, u8); 29] = [
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 0),
    (7, 0),
    (8, 0),
    (9, 0),
    (10, 0),
    (11, 1),
    (13, 1),
    (15, 1),
    (17, 1),
    (19, 2),
    (23, 2),
    (27, 2),
    (31, 2),
    (35, 3),
    (43, 3),
    (51, 3),
    (59, 3),
    (67, 4),
    (83, 4),
    (99, 4),
    (115, 4),
    (131, 5),
    (163, 5),
    (195, 5),
    (227, 5),
    (258, 0),
];

/// Distance-code table: `(base_distance, extra_bits)` for codes 0..=29.
/// Codes 0..=28 match RFC 1951; code 29 is the Deflate64 extension covering
/// 32 769..=65 536 (13 extra bits, base 32 769) — the key difference from
/// standard DEFLATE, which stops at 32 768.
const DISTANCE_TABLE: [(u32, u8); 30] = [
    (1, 0),
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 1),
    (7, 1),
    (9, 2),
    (13, 2),
    (17, 3),
    (25, 3),
    (33, 4),
    (49, 4),
    (65, 5),
    (97, 5),
    (129, 6),
    (193, 6),
    (257, 7),
    (385, 7),
    (513, 8),
    (769, 8),
    (1025, 9),
    (1537, 9),
    (2049, 10),
    (3073, 10),
    (4097, 11),
    (6145, 11),
    (8193, 12),
    (12_289, 12),
    (16_385, 13),
    // Deflate64 extension: code 29 carries 13 extra bits, extending the
    // range to 16 385 + (1 << 13) - 1 = 65 536 — the full 64 KB window.
    (32_769, 13),
];

/// Decompose a match length into `(code, extra_value, extra_bits)`.
#[must_use]
pub fn length_encode(length: usize) -> (u16, u32, u8) {
    for (idx, (base, extra_bits)) in LENGTH_TABLE.iter().enumerate() {
        let base_len = usize::from(*base);
        let max_for_code = base_len + ((1u32 << extra_bits) - 1) as usize;
        if length <= max_for_code {
            let code = 257u16 + idx as u16;
            let extra = (length - base_len) as u32;
            return (code, extra, *extra_bits);
        }
    }
    // length == 258 maps to code 285 with 0 extra bits.
    (285, 0, 0)
}

/// Reconstruct a match length from a code and its extra-bit value.
#[must_use]
pub fn length_decode(code: u16, extra: u32) -> usize {
    if !(257..=285).contains(&code) {
        return MAX_MATCH_LENGTH;
    }
    let idx = (code - 257) as usize;
    let (base, _) = LENGTH_TABLE[idx];
    usize::from(base) + extra as usize
}

/// Decompose a distance into `(code, extra_value, extra_bits)`.
#[must_use]
pub fn distance_encode(distance: usize) -> (u8, u32, u8) {
    for (idx, (base, extra_bits)) in DISTANCE_TABLE.iter().enumerate() {
        let base_d = *base as usize;
        let max_for_code = base_d + ((1u32 << extra_bits) - 1) as usize;
        if distance <= max_for_code {
            let extra = (distance - base_d) as u32;
            return (idx as u8, extra, *extra_bits);
        }
    }
    // Distances > 65 536 are clamped to the Deflate64 maximum.
    (29, (1u32 << 13) - 1, 13)
}

/// Reconstruct a distance from a code and its extra-bit value.
#[must_use]
pub fn distance_decode(code: u8, extra: u32) -> usize {
    if usize::from(code) >= DISTANCE_TABLE.len() {
        return 1;
    }
    let (base, _) = DISTANCE_TABLE[usize::from(code)];
    base as usize + extra as usize
}

/// Number of extra bits associated with a length code (0 if invalid).
#[must_use]
pub fn length_extra_bits(code: u16) -> u8 {
    if !(257..=285).contains(&code) {
        return 0;
    }
    LENGTH_TABLE[(code - 257) as usize].1
}

/// Number of extra bits associated with a distance code (0 if invalid).
#[must_use]
pub fn distance_extra_bits(code: u8) -> u8 {
    if usize::from(code) >= DISTANCE_TABLE.len() {
        return 0;
    }
    DISTANCE_TABLE[usize::from(code)].1
}

/// A Huffman code: the bit pattern and its length.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HuffCode {
    /// Bit pattern stored MSB-first: the first transmitted bit is the
    /// most significant set bit within `len` bits.
    pub bits: u32,
    /// Number of bits in the code.
    pub len: u8,
}

/// A Huffman code table: symbol → code.
///
/// Built canonically from a frequency map. Deterministic: the same frequency
/// distribution always yields the same code assignment.
#[derive(Clone, Debug, Default)]
pub struct HuffTable {
    /// `(symbol, code)` pairs, sorted by symbol for deterministic serialise.
    codes: Vec<(u16, HuffCode)>,
}

impl HuffTable {
    /// Build a Huffman table from a symbol → frequency map.
    ///
    /// Port of the Ruby `build_tree` + `generate_codes` pair: repeatedly
    /// merge the two lowest-frequency nodes, then walk the tree assigning
    /// `0`/`1` to left/right edges. Ties broken by minimum leaf symbol so
    /// the result is independent of merge order.
    #[must_use]
    pub fn from_frequencies(freqs: &[(u16, u64)]) -> Self {
        if freqs.is_empty() {
            return Self { codes: Vec::new() };
        }

        let mut nodes: Vec<Node> = freqs
            .iter()
            .filter(|(_, f)| *f > 0)
            .map(|(sym, freq)| Node {
                symbol: Some(*sym),
                freq: *freq,
                left: None,
                right: None,
            })
            .collect();
        nodes.sort_by(|a, b| a.freq.cmp(&b.freq).then_with(|| a.symbol.cmp(&b.symbol)));

        if nodes.is_empty() {
            return Self { codes: Vec::new() };
        }

        // Special case: a single distinct symbol gets a 1-bit code so it has
        // a valid prefix for decoding.
        if nodes.len() == 1 {
            let sym = nodes[0].symbol.unwrap_or(0);
            return Self {
                codes: vec![(sym, HuffCode { bits: 0, len: 1 })],
            };
        }

        while nodes.len() > 1 {
            nodes.sort_by(|a, b| {
                a.freq
                    .cmp(&b.freq)
                    .then_with(|| node_symbol_key(a).cmp(&node_symbol_key(b)))
            });
            let left = nodes.remove(0);
            let right = nodes.remove(0);
            nodes.push(Node {
                symbol: None,
                freq: left.freq + right.freq,
                left: Some(Box::new(left)),
                right: Some(Box::new(right)),
            });
        }

        let mut codes: Vec<(u16, HuffCode)> = Vec::new();
        generate_codes(&nodes[0], 0, 0, &mut codes);
        codes.sort_by_key(|(s, _)| *s);
        Self { codes }
    }

    /// Look up the code for a symbol, if present.
    #[must_use]
    pub fn code_for(&self, symbol: u16) -> Option<HuffCode> {
        self.codes
            .iter()
            .find(|(s, _)| *s == symbol)
            .map(|(_, c)| *c)
    }

    /// Build an inverse table for decoding (bit pattern → symbol).
    #[must_use]
    pub fn invert(&self) -> InverseTable {
        let mut entries: Vec<(HuffCode, u16)> = self.codes.iter().map(|(s, c)| (*c, *s)).collect();
        // Shortest codes first so the decoder's scan matches the most
        // specific (shortest) prefix first.
        entries.sort_by(|a, b| (a.0.len, a.0.bits).cmp(&(b.0.len, b.0.bits)));
        InverseTable { entries }
    }

    /// Iterate over `(symbol, code)` pairs in symbol order.
    pub fn iter(&self) -> impl Iterator<Item = (u16, HuffCode)> + '_ {
        self.codes.iter().copied()
    }

    /// Serialise: `count: u16` then `count` × `(symbol: u16, bits: u32, len: u8)`.
    /// Stores the actual bit patterns so deserialisation is exact.
    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 + self.codes.len() * 7);
        let count = u16::try_from(self.codes.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&count.to_be_bytes());
        for (sym, code) in &self.codes {
            out.extend_from_slice(&sym.to_be_bytes());
            out.extend_from_slice(&code.bits.to_be_bytes());
            out.push(code.len);
        }
        out
    }

    /// Deserialise a table written by [`Self::serialize`], advancing
    /// `*offset` past the consumed bytes.
    pub fn deserialize(buf: &[u8], offset: &mut usize) -> Option<Self> {
        /// Bytes per serialised entry: `symbol: u16` + `bits: u32` + `len: u8`.
        const ENTRY: usize = 2 + 4 + 1;
        if *offset + 2 > buf.len() {
            return None;
        }
        let count = u16::from_be_bytes([buf[*offset], buf[*offset + 1]]) as usize;
        *offset += 2;
        if *offset + count * ENTRY > buf.len() {
            return None;
        }
        let mut codes = Vec::with_capacity(count);
        for _ in 0..count {
            let sym = u16::from_be_bytes([buf[*offset], buf[*offset + 1]]);
            let bits = u32::from_be_bytes([
                buf[*offset + 2],
                buf[*offset + 3],
                buf[*offset + 4],
                buf[*offset + 5],
            ]);
            let len = buf[*offset + 6];
            *offset += ENTRY;
            codes.push((sym, HuffCode { bits, len }));
        }
        codes.sort_by_key(|(s, _)| *s);
        Some(Self { codes })
    }
}

/// Inverted Huffman table for decoding: maps a `(len, bits)` prefix to a
/// symbol. Linear scan suffices — codes are short (≤ ~16 bits) and few.
#[derive(Clone, Debug, Default)]
pub struct InverseTable {
    entries: Vec<(HuffCode, u16)>,
}

impl InverseTable {
    /// Decode one symbol from the bit reader. Returns the symbol and
    /// advances `pos` past the consumed bits, or `None` on exhaustion.
    pub fn decode_symbol(&self, bits: &[u8], pos: &mut usize) -> Option<u16> {
        let mut acc: u32 = 0;
        let mut len: u8 = 0;
        while *pos < bits.len() {
            acc = (acc << 1) | u32::from(bits[*pos]);
            *pos += 1;
            len += 1;
            for (code, sym) in &self.entries {
                if code.len == len && code.bits == acc {
                    return Some(*sym);
                }
            }
            if len >= 24 {
                return None;
            }
        }
        None
    }
}

/// Tree node used during construction.
struct Node {
    symbol: Option<u16>,
    freq: u64,
    left: Option<Box<Node>>,
    right: Option<Box<Node>>,
}

/// Tie-breaker for sorting internal nodes: the minimum leaf symbol in the
/// subtree, so the construction order is independent of merge sequence.
fn node_symbol_key(node: &Node) -> u16 {
    min_leaf(node).unwrap_or(0)
}

#[allow(clippy::min_ident_chars)]
fn min_leaf(node: &Node) -> Option<u16> {
    match (node.symbol, node.left.as_deref(), node.right.as_deref()) {
        (Some(symbol), None, None) => Some(symbol),
        (_, left, right) => {
            let left_val = left.and_then(min_leaf);
            let right_val = right.and_then(min_leaf);
            match (left_val, right_val) {
                (Some(lv), Some(rv)) => Some(lv.min(rv)),
                (Some(lv), None) => Some(lv),
                (None, Some(rv)) => Some(rv),
                (None, None) => None,
            }
        }
    }
}

/// Walk the tree depth-first assigning codes. Left edge = `0`, right = `1`.
fn generate_codes(node: &Node, bits: u32, len: u8, out: &mut Vec<(u16, HuffCode)>) {
    if let Some(sym) = node.symbol {
        out.push((sym, HuffCode { bits, len }));
        return;
    }
    if let Some(l) = &node.left {
        generate_codes(l, bits << 1, len + 1, out);
    }
    if let Some(r) = &node.right {
        generate_codes(r, (bits << 1) | 1, len + 1, out);
    }
}
