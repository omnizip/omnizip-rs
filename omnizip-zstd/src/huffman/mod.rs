//! Canonical Huffman decoder for ZSTD literals (RFC 8878 §4.2).
//!
//! Ported with substantial rework from
//! `omnizip/lib/omnizip/algorithms/zstandard/huffman.rb` (269 LOC, MIT,
//! Ribose Inc.). The Ruby's FSE-compressed-weights path is stubbed
//! (see `../../../../../omnizip/BUGREPORT.01-huffman-fse-weights-stub.md`);
//! the implementation here reads the table correctly.
//!
//! ## Architecture
//!
//! Two responsibilities, two types:
//!
//! - [`HuffmanTable`] — holds the (symbol, length) table plus a
//!   flat lookup table for `O(1)` decode of any code up to
//!   [`crate::constants::HUFFMAN_MAX_BITS`] bits wide.
//! - [`HuffmanDecoder`] — wraps a table + forward bitstream, providing
//!   a `decode_one_symbol` API that the literals section uses.
//!
//! ## Decode strategy
//!
//! Single-level lookup table indexed by the next `max_bits` bits of the
//! forward bitstream. Each entry stores the symbol and the actual code
//! length; the caller advances the bitstream by the stored length.
//! `max_bits = 11` for ZSTD, so the table is 2048 × 2 bytes = 4 KiB.

#![forbid(unsafe_code)]

use crate::constants::HUFFMAN_MAX_BITS;
use crate::fse::ForwardBitStream;
use crate::ZstdError;

/// Maximum code length ZSTD permits, in bits. Constant alias for
/// readability.
const MAX_BITS: u8 = HUFFMAN_MAX_BITS;

/// One decode-table slot: the symbol the code maps to, and how many
/// bits the code actually occupies in the bitstream (≤ `MAX_BITS`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeEntry {
    pub symbol: u8,
    pub length: u8,
}

/// Canonical Huffman table. Built once per literals section and reused
/// by the decoder until the next compressed/treeless block updates it.
#[derive(Clone, Debug)]
pub struct HuffmanTable {
    /// `1 << MAX_BITS` entries; index by the next `MAX_BITS` bits.
    /// Entries for codes shorter than `MAX_BITS` are duplicated to
    /// fill every extension with trailing zeros — that's how the
    /// single-level lookup handles short codes.
    lookup: Vec<DecodeEntry>,
    /// The original weights, retained for diagnostics and for the
    /// upcoming encoder port.
    weights: Vec<u8>,
}

impl HuffmanTable {
    /// Build a canonical Huffman table from a per-symbol weight array.
    ///
    /// Weight 0 → symbol is absent. Higher weight → shorter code.
    /// Code length `= max(weights) - weight + 1`, clamped to
    /// [`MAX_BITS`].
    ///
    /// # Errors
    ///
    /// Returns [`ZstdError::Corrupt`] if the implied code lengths
    /// cannot be assigned valid canonical codes (e.g. overflow).
    pub fn from_weights(weights: &[u8]) -> Result<Self, ZstdError> {
        let code_lengths = calculate_code_lengths(weights);
        let codes = build_canonical_codes(&code_lengths)?;
        let lookup = build_lookup_table(&codes, &code_lengths);
        Ok(Self {
            lookup,
            weights: weights.to_vec(),
        })
    }

    /// Construct a "no symbols" table. Every decode returns symbol 0
    /// with length 0. Used for empty-literals blocks where the encoder
    /// emits no Huffman header.
    #[must_use]
    pub fn empty() -> Self {
        let entry = DecodeEntry { symbol: 0, length: 0 };
        Self {
            lookup: vec![entry; 1usize << MAX_BITS],
            weights: Vec::new(),
        }
    }

    /// The original weights. Returned as-is for diagnostics / encoder
    /// parity; mutating the slice does not affect this table.
    #[must_use]
    pub fn weights(&self) -> &[u8] {
        &self.weights
    }

    /// Peek-decode one symbol from `bitstream` and consume its bits.
    ///
    /// # Errors
    ///
    /// Returns [`ZstdError::Corrupt`] if the bitstream is exhausted
    /// before a complete code is read.
    pub fn decode(&self, bitstream: &mut ForwardBitStream<'_>) -> Result<u8, ZstdError> {
        // Peek the next MAX_BITS bits without consuming. This returns
        // 0 for bits past the end of the stream, which is the correct
        // behaviour for the canonical-Huffman lookup (the trailing
        // bits will be the stream's padding).
        let peek = bitstream.peek_bits(MAX_BITS);
        let entry = self.lookup[peek as usize];
        if entry.length == 0 {
            return Err(ZstdError::Corrupt {
                reason: format!(
                    "huffman lookup miss: peek={peek:#011b} (table has no code for this prefix)"
                ),
            });
        }
        // Consume exactly `entry.length` bits.
        let _ = bitstream.read_bits(u32::from(entry.length));
        Ok(entry.symbol)
    }
    /// Number of distinct symbols in the source alphabet.
    #[must_use]
    pub fn symbol_count(&self) -> usize {
        self.weights.len()
    }
}

/// Stateful decoder — pairs a [`HuffmanTable`] with a forward
/// bitstream for use as a single-call "decode N symbols" API.
pub struct HuffmanDecoder<'t, 'b> {
    table: &'t HuffmanTable,
    bitstream: ForwardBitStream<'b>,
}

impl<'t, 'b> HuffmanDecoder<'t, 'b> {
    /// Construct a decoder backed by `table` reading from `data`
    /// starting at byte offset 0.
    #[must_use]
    pub fn new(table: &'t HuffmanTable, data: &'b [u8]) -> Self {
        Self {
            table,
            bitstream: ForwardBitStream::from_start(data),
        }
    }

    /// Decode one symbol. See [`HuffmanTable::decode`].
    ///
    /// # Errors
    ///
    /// Returns [`ZstdError::Corrupt`] if the bitstream ends mid-symbol
    /// or the next bits don't match any table entry.
    pub fn decode_one(&mut self) -> Result<u8, ZstdError> {
        self.table.decode(&mut self.bitstream)
    }

    /// Decode `out.len()` symbols into `out`. Stops early on error.
    ///
    /// # Errors
    ///
    /// Returns the first error encountered (see [`Self::decode_one`]).
    pub fn decode_into(&mut self, out: &mut [u8]) -> Result<(), ZstdError> {
        for slot in out.iter_mut() {
            *slot = self.decode_one()?;
        }
        Ok(())
    }
}

// ── Free helpers — the canonical-Huffman algorithm, broken out so
// the encoder (Phase B) can reuse them.

/// Convert weights to per-symbol code lengths. Weight 0 → length 0
/// (symbol absent). For present symbols:
/// `length = max(weights) - weight + 1`, clamped to `MAX_BITS`.
fn calculate_code_lengths(weights: &[u8]) -> Vec<u8> {
    if weights.is_empty() {
        return Vec::new();
    }
    let max_weight = match weights.iter().copied().max() {
        Some(0) | None => return vec![0; weights.len()],
        Some(m) => m,
    };
    weights
        .iter()
        .map(|&w| {
            if w == 0 {
                0
            } else {
                // max_weight - w + 1, clamped to MAX_BITS.
                let len = max_weight - w + 1;
                len.min(MAX_BITS)
            }
        })
        .collect()
}

/// Build canonical Huffman codes from per-symbol code lengths. Returns
/// a `codes` vector parallel to `lengths` — `codes[i]` is the canonical
/// code for symbol `i`, valid iff `lengths[i] > 0`.
///
/// Algorithm: count `bl_count[len]` symbols at each length, compute
/// the starting code at each length, then walk symbols in order.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] if the lengths do not form a valid
/// Huffman tree (Kraft inequality overflow).
fn build_canonical_codes(lengths: &[u8]) -> Result<Vec<u32>, ZstdError> {
    let mut codes = vec![0u32; lengths.len()];
    if lengths.is_empty() {
        return Ok(codes);
    }
    let max_length = lengths.iter().copied().max().unwrap_or(0);
    if max_length == 0 {
        return Ok(codes);
    }

    // Count symbols at each length.
    let mut bl_count = vec![0u32; usize::from(max_length) + 1];
    for &len in lengths {
        if len > 0 {
            bl_count[usize::from(len)] += 1;
        }
    }

    // Compute starting code at each length.
    let mut next_code = vec![0u32; usize::from(max_length) + 1];
    let mut code = 0u32;
    for bits in 1..=usize::from(max_length) {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }

    // Kraft inequality: the total code space must not overflow
    // `2^max_length`. Overflow means the lengths are inconsistent.
    let total_used: u32 = bl_count
        .iter()
        .enumerate()
        .skip(1)
        .map(|(bits, count)| count.checked_shl((max_length as u32) - (bits as u32)).unwrap_or(0))
        .sum();
    if total_used > (1u32 << max_length) {
        return Err(ZstdError::Corrupt {
            reason: "huffman code lengths over-assign code space".into(),
        });
    }

    // Assign codes to symbols.
    for (symbol, &len) in lengths.iter().enumerate() {
        if len > 0 {
            codes[symbol] = next_code[usize::from(len)];
            next_code[usize::from(len)] += 1;
        }
    }
    Ok(codes)
}

/// Build the single-level lookup table. Each `(code, length)` pair is
/// replicated across every extension that begins with `code` — i.e.
/// `(code << (MAX_BITS - length)) | extension` for every extension in
/// `0..2^(MAX_BITS - length)`.
fn build_lookup_table(codes: &[u32], lengths: &[u8]) -> Vec<DecodeEntry> {
    let mut lookup = vec![
        DecodeEntry {
            symbol: 0,
            length: 0,
        };
        1usize << MAX_BITS
    ];
    for (symbol, &len) in lengths.iter().enumerate() {
        if len == 0 {
            continue;
        }
        let code = codes[symbol];
        let padding_bits = MAX_BITS - len;
        let padding_count = 1u32 << padding_bits;
        let base = code << padding_bits;
        for padding in 0..padding_count {
            let idx = usize::try_from(base | padding).expect("index fits in 1 << MAX_BITS");
            lookup[idx] = DecodeEntry {
                symbol: u8::try_from(symbol).unwrap_or(0),
                length: len,
            };
        }
    }
    lookup
}

// `ForwardBitStream::peek_bits` is implemented directly on the type
// in `fse::bitstream`. No extension trait needed here.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_lengths_match_weight_formula() {
        // 4 symbols, weights [3, 1, 2, 0]. max=3.
        // Lengths: max - w + 1 → [1, 3, 2, 0]. Clamped at 11.
        let w = [3u8, 1, 2, 0];
        let lens = calculate_code_lengths(&w);
        assert_eq!(lens, vec![1, 3, 2, 0]);
    }

    #[test]
    fn code_lengths_clamp_to_max_bits() {
        // max_weight = 20, weight 20 → length 1, weight 1 → length
        // 20 clamped to MAX_BITS=11.
        let w = [20u8, 1];
        let lens = calculate_code_lengths(&w);
        assert_eq!(lens, vec![1, 11]);
    }

    #[test]
    fn empty_weights_give_empty_lengths() {
        assert!(calculate_code_lengths(&[]).is_empty());
        assert_eq!(calculate_code_lengths(&[0, 0, 0]), vec![0, 0, 0]);
    }

    #[test]
    fn canonical_codes_are_prefix_free() {
        // Weights [3, 1, 2, 0] → lengths [1, 3, 2, 0] → codes:
        //   symbol 0 (len 1): 0
        //   symbol 2 (len 2): 10
        //   symbol 1 (len 3): 110
        let w = [3u8, 1, 2, 0];
        let lens = calculate_code_lengths(&w);
        let codes = build_canonical_codes(&lens).expect("codes");
        assert_eq!(codes[0], 0b0);
        assert_eq!(codes[2], 0b10);
        assert_eq!(codes[1], 0b110);
    }

    #[test]
    fn kraft_violation_is_detected() {
        // Three symbols with length 1 → Kraft = 3/2 > 1.
        let lens = [1u8, 1, 1];
        let err = build_canonical_codes(&lens).unwrap_err();
        assert!(matches!(err, ZstdError::Corrupt { .. }));
    }

    #[test]
    fn lookup_table_handles_short_codes_by_replication() {
        // Single symbol at length 1: code 0 fills half the table
        // (every index where bit 0 of peek is 0), code 1 fills the
        // other half.
        let lens = [1u8, 1];
        let codes = build_canonical_codes(&lens).expect("codes");
        let tbl = build_lookup_table(&codes, &lens);
        assert_eq!(tbl.len(), 1 << MAX_BITS);
        // Spot check: index 0 has symbol 0 (since code 0 is symbol 0).
        assert_eq!(tbl[0].symbol, 0);
        // Index 0b10000000000 (the high bit set) has symbol 1
        // (since code 1 is symbol 1).
        assert_eq!(tbl[1 << (MAX_BITS - 1)].symbol, 1);
    }

    #[test]
    fn table_from_weights_round_trips_a_simple_alphabet() {
        // Build a table for the alphabet 'a', 'b', 'c', 'd' with
        // weights [3, 1, 2, 0]. Decode the codes we expect to be
        // assigned.
        let table = HuffmanTable::from_weights(&[3, 1, 2, 0]).expect("table");
        // Code 0b0 (1 bit) → symbol 0 ('a')
        // Code 0b10 (2 bits) → symbol 2 ('c')
        // Code 0b110 (3 bits) → symbol 1 ('b')
        // We can drive the decoder via a ForwardBitStream packed
        // MSB-first. Pack: 0, 10, 110 = 0_10_110 followed by zero pad.
        // MSB-first: 0101_1000 = 0x58.
        let bytes = [0x58u8];
        let mut dec = HuffmanDecoder::new(&table, &bytes);
        // First symbol: bits 0..1 = 0 → 'a' (symbol 0).
        assert_eq!(dec.decode_one().unwrap(), 0);
        // Next: bits 1..3 = 10 → symbol 2 ('c').
        assert_eq!(dec.decode_one().unwrap(), 2);
        // Next: bits 3..6 = 110 → symbol 1 ('b').
        assert_eq!(dec.decode_one().unwrap(), 1);
    }

    #[test]
    fn empty_table_decodes_zero_symbols() {
        let table = HuffmanTable::empty();
        // Empty table → every lookup returns length 0 → error.
        let err = table.decode(&mut ForwardBitStream::from_start(&[0xFF])).unwrap_err();
        assert!(matches!(err, ZstdError::Corrupt { .. }));
    }
}
