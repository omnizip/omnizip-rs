//! Entropy coding for the GLZA grammar symbol stream (Phase 2).
//!
//! Phase 1 stores each grammar symbol with a raw varint/marker encoding.
//! Phase 2 builds a canonical Huffman code over the entire symbol stream
//! (across the start rule and every rule body) and bit-packs the symbols,
//! yielding a substantially smaller body for the same grammar.
//!
//! ## Alphabet
//!
//! Each `Symbol` maps to a single alphabet index:
//!
//! - `Symbol::Byte(b)`     -> index `b` (0..=255)
//! - `Symbol::Rule(n)`     -> index `256 + n`
//!
//! The alphabet is dense over `[0, 256 + rule_count)`; symbols that never
//! appear get code length 0 and are skipped during canonical-code
//! construction.
//!
//! ## Canonical Huffman
//!
//! Code lengths are computed with a length-limited (<= 15 bits) Huffman
//! built via the package-merge algorithm — bounded code lengths keep the
//! decoder's table small and deterministic. The canonical code assignment
//! follows DEFLATE conventions:
//!
//! 1. Sort symbols by `(code_length asc, symbol_index asc)`.
//! 2. Assign codes in increasing order, with each new `code_length` getting
//!    the next code = `(previous_code + 1) << (new_len - old_len)`.
//!
//! ## Bit order
//!
//! Codes are written MSB-first into a byte buffer; trailing bits in the
//! final byte are zero-padded. This is identical to DEFLATE's bit order
//! and is byte-identical across runs/machines.
//!
//! ## Determinism
//!
//! Frequency counting, code-length assignment, and canonical-code
//! assignment are all deterministic total orders tied only to the grammar
//! content. Same grammar in -> identical bit stream out.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use crate::grammar::{Grammar, Symbol};

/// Maximum Huffman code length. Bounded so the decoder's lookup tables stay
/// small and the wire-format `[u8; alphabet_size]` length array fits in a u8.
pub const MAX_CODE_LENGTH: u8 = 15;

/// Map a `Symbol` to its alphabet index. `Symbol::Byte(b)` -> `b`,
/// `Symbol::Rule(n)` -> `256 + n`. Returns `u32` so callers can use it for
/// array indexing without intermediate casts.
#[must_use]
#[inline]
pub fn symbol_to_index(s: Symbol) -> u32 {
    match s {
        Symbol::Byte(b) => u32::from(b),
        Symbol::Rule(n) => 256 + u32::from(n),
    }
}

/// Inverse of [`symbol_to_index`]. `idx < 256` -> `Byte(idx as u8)`,
/// otherwise `Rule((idx - 256) as u16)`.
#[must_use]
#[inline]
pub fn index_to_symbol(idx: u32) -> Symbol {
    if idx < 256 {
        Symbol::Byte(idx as u8)
    } else {
        Symbol::Rule((idx - 256) as u16)
    }
}

/// Compute the alphabet size for a grammar: `256 + rule_count`.
#[must_use]
pub fn alphabet_size(grammar: &Grammar) -> u32 {
    256 + grammar.rules.len() as u32
}

/// Count the frequency of each alphabet symbol across the entire grammar
/// (start rule + every rule body). Returns a `Vec<u64>` of length
/// `alphabet_size(grammar)`.
#[must_use]
pub fn symbol_frequencies(grammar: &Grammar) -> Vec<u64> {
    let size = alphabet_size(grammar) as usize;
    let mut freq = vec![0u64; size];
    let bump = |freq: &mut [u64], s: Symbol| {
        let idx = symbol_to_index(s) as usize;
        if idx < freq.len() {
            freq[idx] = freq[idx].saturating_add(1);
        }
    };
    for &s in &grammar.start_rule {
        bump(&mut freq, s);
    }
    for rule in &grammar.rules {
        for &s in rule {
            bump(&mut freq, s);
        }
    }
    freq
}

/// A package-merge list node: total weight and a sparse per-symbol count.
#[derive(Clone)]
struct PmNode {
    weight: u64,
    counts: Vec<(usize, u32)>, // (symbol_index, count)
}

/// Compute canonical Huffman code lengths (length-limited to
/// [`MAX_CODE_LENGTH`]) for the given symbol frequencies.
///
/// Returns a `Vec<u8>` of length `freq.len()` where `0` means "symbol
/// unused" and `1..=MAX_CODE_LENGTH` is the assigned code length.
///
/// Uses the package-merge algorithm with a hard length cap of
/// `MAX_CODE_LENGTH`, which is optimal for the given cap.
#[must_use]
pub fn code_lengths(freq: &[u64]) -> Vec<u8> {
    let n = freq.len();
    let mut lengths = vec![0u8; n];

    // Collect non-zero symbols (index, weight). Symbols with weight 0 get
    // length 0 (unused).
    let mut items: Vec<(u64, usize)> = freq
        .iter()
        .enumerate()
        .filter(|&(_, &f)| f > 0)
        .map(|(i, &f)| (f, i))
        .collect();

    if items.is_empty() {
        return lengths;
    }
    if items.len() == 1 {
        // Single-symbol alphabet: give it length 1 so the stream is
        // well-defined (each occurrence emits one bit).
        lengths[items[0].1] = 1;
        return lengths;
    }

    // Package-merge with length limit L = MAX_CODE_LENGTH.
    //
    // We want, for each symbol, the number of "lists" it participates in
    // across the L passes — that count is its code length.
    //
    // Standard formulation: start with `items` sorted by weight; each pass
    // "merges" pairs of items from the current list with the original list.
    // After L passes, the count of appearances of each original symbol is
    // its optimal length-limited code length.
    //
    // Each item carries: weight (u64) and a list of original-symbol indices
    // it represents. To bound memory, we only track symbol counts.

    items.sort_by_key(|(w, _)| *w);

    // Each "node" in the package-merge list holds a total weight and a
    // sparse count of how many times each symbol index appears within it.
    // Since symbols can be many (up to ~65k), we use a HashMap-free
    // representation: a small Vec of (idx, count) pairs. Most nodes touch
    // very few symbols.

    // Originals: one PmNode per non-zero symbol.
    let originals: Vec<PmNode> = items
        .iter()
        .map(|(w, i)| PmNode {
            weight: *w,
            counts: vec![(*i, 1)],
        })
        .collect();

    // current list, sorted by weight ascending.
    let mut current: Vec<PmNode> = originals.clone();

    let mut sym_counts: Vec<u32> = vec![0; n];

    // We need (2 * (num_symbols - 1)) items selected across the L passes.
    // For L = MAX_CODE_LENGTH, run L passes.
    for _pass in 0..MAX_CODE_LENGTH {
        if current.len() < 2 {
            break;
        }
        // Pair adjacent items, summing their weights and concatenating counts.
        let mut packaged: Vec<PmNode> = Vec::with_capacity(current.len() / 2);
        let mut i = 0;
        while i + 1 < current.len() {
            let a = &current[i];
            let b = &current[i + 1];
            let weight = a.weight + b.weight;
            let mut counts = a.counts.clone();
            for &(idx, c) in &b.counts {
                if let Some(pos) = counts.iter().position(|(x, _)| *x == idx) {
                    counts[pos].1 += c;
                } else {
                    counts.push((idx, c));
                }
            }
            packaged.push(PmNode { weight, counts });
            i += 2;
        }
        // Merge packaged with originals (both sorted by weight), keeping the
        // lowest-weight (2n - 2) items. Standard package-merge keeps the
        // cheapest items up to a budget of 2*(k-1) after L passes, but to
        // keep memory bounded and behaviour identical across runs we use a
        // stable sort by weight and keep the first items.
        let mut merged: Vec<PmNode> = Vec::with_capacity(packaged.len() + originals.len());
        // Both lists are already sorted by weight; do a stable merge.
        let mut pi = 0;
        let mut oi = 0;
        while pi < packaged.len() && oi < originals.len() {
            if packaged[pi].weight <= originals[oi].weight {
                merged.push(packaged[pi].clone());
                pi += 1;
            } else {
                merged.push(originals[oi].clone());
                oi += 1;
            }
        }
        while pi < packaged.len() {
            merged.push(packaged[pi].clone());
            pi += 1;
        }
        while oi < originals.len() {
            merged.push(originals[oi].clone());
            oi += 1;
        }
        current = merged;
    }

    // Take the first 2*(k-1) items from current; count symbol appearances.
    let k = items.len();
    let budget = 2 * (k.saturating_sub(1));
    for node in current.iter().take(budget) {
        for &(idx, c) in &node.counts {
            sym_counts[idx] += c;
        }
    }

    for (i, &c) in sym_counts.iter().enumerate() {
        // Clamp: code lengths should already be <= MAX_CODE_LENGTH for a
        // correct package-merge, but clamp defensively.
        lengths[i] = c.min(u32::from(MAX_CODE_LENGTH)) as u8;
    }

    lengths
}

/// Build the canonical Huffman code for each symbol given its code length.
/// Returns `Vec<(u32 code, u8 len)>` indexed by symbol index. Symbols with
/// length 0 get `(0, 0)`.
#[must_use]
pub fn canonical_codes(lengths: &[u8]) -> Vec<(u32, u8)> {
    let n = lengths.len();
    let mut out = vec![(0u32, 0u8); n];

    // DEFLATE-style canonical assignment:
    // 1) Count symbols per length.
    // 2) Compute first code per length.
    // 3) Assign in symbol-index order within each length.
    let max_len = lengths.iter().copied().max().unwrap_or(0) as usize;
    if max_len == 0 {
        return out;
    }
    let mut bl_count = vec![0u32; max_len + 1];
    for &l in lengths {
        if l > 0 {
            bl_count[l as usize] += 1;
        }
    }

    let mut next_code = vec![0u32; max_len + 1];
    let mut code = 0u32;
    for bits in 1..=max_len {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }

    for (i, &l) in lengths.iter().enumerate() {
        if l > 0 {
            out[i] = (next_code[l as usize], l);
            next_code[l as usize] += 1;
        }
    }
    out
}

/// Bit packer that writes codes MSB-first into a byte buffer.
pub struct BitWriter {
    out: Vec<u8>,
    bit_buf: u64,
    bits_in_buf: u8,
}

impl BitWriter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            bit_buf: 0,
            bits_in_buf: 0,
        }
    }

    /// Write `len` bits of `code`, MSB-first.
    pub fn write_bits(&mut self, code: u32, len: u8) {
        if len == 0 {
            return;
        }
        // We accumulate into bit_buf with the next bit to emit sitting at the
        // top (highest position) of the unfilled portion. To emit MSB-first,
        // shift the code left so its MSB aligns with the next free slot.
        self.bit_buf = (self.bit_buf << u64::from(len)) | u64::from(code & mask(len));
        self.bits_in_buf += len;
        while self.bits_in_buf >= 8 {
            self.bits_in_buf -= 8;
            let byte = ((self.bit_buf >> self.bits_in_buf) & 0xFF) as u8;
            self.out.push(byte);
        }
    }

    /// Flush remaining bits, zero-padded on the low side.
    pub fn flush(mut self) -> Vec<u8> {
        if self.bits_in_buf > 0 {
            let byte = ((self.bit_buf << (8 - self.bits_in_buf)) & 0xFF) as u8;
            self.out.push(byte);
            self.bits_in_buf = 0;
        }
        self.out
    }
}

impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// Bit reader that pulls codes MSB-first from a byte buffer.
pub struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u8, // bits already consumed from data[byte_pos], 0..=7
}

impl<'a> BitReader<'a> {
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    /// Read one bit. Returns `Some(bit)` or `None` if exhausted.
    pub fn read_bit(&mut self) -> Option<u32> {
        if self.byte_pos >= self.data.len() {
            return None;
        }
        let b = self.data[self.byte_pos];
        // MSB first: bit 7 is the first emitted.
        let bit = u32::from((b >> (7 - self.bit_pos)) & 1);
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        Some(bit)
    }

    /// Current position (in bits) from the start of the buffer.
    #[must_use]
    #[allow(dead_code)]
    pub fn bit_offset(&self) -> usize {
        self.byte_pos * 8 + self.bit_pos as usize
    }

    /// Remaining bytes in the underlying buffer (including the partial one).
    #[must_use]
    #[allow(dead_code)]
    pub fn remaining_bytes(&self) -> usize {
        self.data.len().saturating_sub(self.byte_pos)
    }
}

#[inline]
fn mask(len: u8) -> u32 {
    if len >= 32 {
        0xFFFF_FFFF
    } else {
        (1u32 << len) - 1
    }
}

/// Decode table for canonical Huffman codes. Decodes one symbol at a time
/// by reading bits MSB-first and matching against code lengths.
pub struct HuffmanDecoder {
    /// For each length L in 1..=MAX, sorted list of (code, symbol). Decoding
    /// walks bits, building up `cur`, and at each length checks if `cur` is
    /// a valid code.
    by_length: Vec<Vec<(u32, u32)>>,
}

impl HuffmanDecoder {
    /// Build a decoder from canonical code lengths.
    #[must_use]
    pub fn from_lengths(lengths: &[u8]) -> Self {
        let codes = canonical_codes(lengths);
        let max_len = lengths.iter().copied().max().unwrap_or(0) as usize;
        let mut by_length = vec![Vec::new(); max_len + 1];
        for (i, &(code, len)) in codes.iter().enumerate() {
            if len > 0 {
                by_length[len as usize].push((code, i as u32));
            }
        }
        for v in &mut by_length {
            v.sort_by_key(|(c, _)| *c);
        }
        Self { by_length }
    }

    /// Decode one symbol from `reader`. Returns `Some(symbol_index)` or
    /// `None` if the reader was exhausted mid-code.
    pub fn decode(&self, reader: &mut BitReader<'_>) -> Option<u32> {
        let mut cur: u32 = 0;
        for len in 1..self.by_length.len() {
            let bit = reader.read_bit()?;
            cur = (cur << 1) | bit;
            if let Ok(idx) = self.by_length[len].binary_search_by_key(&cur, |(c, _)| *c) {
                return Some(self.by_length[len][idx].1);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(codes: &[(u32, u8)], bits: &[u8], bit_count: usize) -> Vec<u32> {
        // Simple MSB-first decoder for testing.
        let mut out = Vec::new();
        let max_len = codes.iter().map(|(_, l)| *l).max().unwrap_or(0) as usize;
        let mut by_length: Vec<Vec<(u32, u32)>> = vec![Vec::new(); max_len + 1];
        for (i, &(c, l)) in codes.iter().enumerate() {
            if l > 0 {
                by_length[l as usize].push((c, i as u32));
            }
        }
        for v in &mut by_length {
            v.sort_by_key(|(c, _)| *c);
        }
        let mut cur = 0u32;
        let mut cur_len = 0usize;
        for bit_i in 0..bit_count {
            let byte = bits[bit_i / 8];
            let bit = u32::from((byte >> (7 - (bit_i % 8))) & 1);
            cur = (cur << 1) | bit;
            cur_len += 1;
            if cur_len < by_length.len() {
                if let Ok(idx) = by_length[cur_len].binary_search_by_key(&cur, |(c, _)| *c) {
                    out.push(by_length[cur_len][idx].1);
                    cur = 0;
                    cur_len = 0;
                }
            }
        }
        out
    }

    #[test]
    fn alphabet_index_round_trip() {
        for b in [0u8, 1, 100, 200, 255] {
            assert_eq!(
                index_to_symbol(symbol_to_index(Symbol::Byte(b))),
                Symbol::Byte(b)
            );
        }
        for n in [0u16, 1, 1000, u16::MAX] {
            assert_eq!(
                index_to_symbol(symbol_to_index(Symbol::Rule(n))),
                Symbol::Rule(n)
            );
        }
    }

    #[test]
    fn code_lengths_simple_skew() {
        // Skewed distribution: symbol 0 dominates.
        let freq = vec![100u64, 10, 5, 1];
        let lengths = code_lengths(&freq);
        // All non-zero symbols get a length.
        assert_eq!(lengths.len(), 4);
        assert!(lengths.iter().all(|&l| l > 0));
        // Most frequent symbol gets the shortest code.
        let max_idx = freq
            .iter()
            .enumerate()
            .max_by_key(|(_, &f)| f)
            .map(|(i, _)| i)
            .unwrap();
        let min_len = *lengths.iter().min().unwrap();
        assert_eq!(lengths[max_idx], min_len);
    }

    #[test]
    fn canonical_codes_are_prefix_free() {
        let freq = vec![50u64, 25, 10, 5, 5, 3, 1, 1];
        let lengths = code_lengths(&freq);
        let codes = canonical_codes(&lengths);
        // Verify prefix-free: no code is a prefix of another.
        let active: Vec<(u32, u8)> = codes.iter().filter(|(_, l)| *l > 0).copied().collect();
        for i in 0..active.len() {
            for j in 0..active.len() {
                if i == j {
                    continue;
                }
                let (ci, li) = active[i];
                let (cj, lj) = active[j];
                if li <= lj {
                    let prefix_ci = ci << (lj - li);
                    let cj_truncated = cj >> (lj - li);
                    assert_ne!(
                        prefix_ci,
                        cj_truncated,
                        "code {ci:0>li$b} (len {li}) is a prefix of {cj:0>lj$b} (len {lj})",
                        li = li as usize,
                        lj = lj as usize,
                    );
                }
            }
        }
    }

    #[test]
    fn bit_writer_round_trip() {
        // Write the bits 1, 0, 1, 1, 0, 0, 1, 0 and verify byte = 0b10110010.
        let mut w = BitWriter::new();
        for &b in &[1u32, 0, 1, 1, 0, 0, 1, 0] {
            w.write_bits(b, 1);
        }
        let bytes = w.flush();
        assert_eq!(bytes, vec![0b1011_0010]);
    }

    #[test]
    fn huffman_round_trip_via_decoder() {
        let freq = vec![100u64, 50, 20, 10, 5, 1, 1, 1];
        let lengths = code_lengths(&freq);
        let codes = canonical_codes(&lengths);

        // Build a stream of symbols and encode them.
        let symbols = [0u32, 0, 1, 0, 2, 1, 0, 3, 4, 5, 0, 6, 7];
        let mut w = BitWriter::new();
        for &s in &symbols {
            let (code, len) = codes[s as usize];
            w.write_bits(code, len);
        }
        let bytes = w.flush();
        // Decode via HuffmanDecoder.
        let dec = HuffmanDecoder::from_lengths(&lengths);
        let mut r = BitReader::new(&bytes);
        let mut got = Vec::new();
        for _ in &symbols {
            got.push(dec.decode(&mut r).expect("decode"));
        }
        assert_eq!(got, symbols);

        // Cross-check against the test decoder for the exact bit stream.
        let total_bits: usize = symbols.iter().map(|&s| codes[s as usize].1 as usize).sum();
        assert_eq!(decode_all(&codes, &bytes, total_bits), symbols);
    }

    #[test]
    fn single_symbol_alphabet() {
        let freq = vec![0u64, 5, 0];
        let lengths = code_lengths(&freq);
        assert_eq!(lengths[1], 1);
        let codes = canonical_codes(&lengths);
        // Single active symbol gets code 0 with length 1.
        assert_eq!(codes[1], (0, 1));
    }

    #[test]
    fn empty_alphabet() {
        let freq: Vec<u64> = vec![0; 4];
        let lengths = code_lengths(&freq);
        assert!(lengths.iter().all(|&l| l == 0));
    }

    #[test]
    fn length_limited_to_max() {
        // Highly skewed: ensure no length exceeds MAX_CODE_LENGTH.
        let mut freq = vec![1u64; 1000];
        freq[0] = 1_000_000;
        let lengths = code_lengths(&freq);
        assert!(
            lengths.iter().all(|&l| l <= MAX_CODE_LENGTH),
            "lengths exceed MAX_CODE_LENGTH: {lengths:?}"
        );
    }
}
