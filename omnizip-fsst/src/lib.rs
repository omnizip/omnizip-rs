//! omnizip-fsst — Pure-Rust FSST (Fast Static Symbol Table) compressor.
//!
//! FSST is a lightweight string compressor that builds a static symbol
//! table from common substrings (1–8 bytes) and replaces occurrences
//! with single-byte escape codes. It's designed as a preprocessor for
//! general-purpose compressors (Brotli, LZ4, ZSTD) — FSST+Brotli beats
//! Brotli alone on text-heavy workloads.
//!
//! ## Algorithm
//!
//! Reference: Boncz, Zukowski, Ven. "FSST: Fast Random Access String
//! Compression" (VLDB 2020).
//!
//! 1. **Train**: Scan input, count 1–8 byte substring frequencies.
//! 2. **Select**: Greedily pick the top 255 symbols by gain
//!    (`(len-1) × count`).
//! 3. **Encode**: Longest-match replace each occurrence with its
//!    escape byte. Byte 255 is the escape-escape: the next byte is a
//!    literal.
//! 4. **Output**: `[symbol_count][symbol_lens][symbol_data][escaped_text]`.
//!
//! ## Wire format
//!
//! ```text
//! +------------------+  1 byte: number of symbols (0–255)
//! | n_symbols        |
//! +------------------+  n bytes: length of each symbol (1–8)
//! | symbol_lens      |
//! +------------------+  sum(symbol_lens) bytes: packed symbol data
//! | symbol_data      |
//! +------------------+  variable: escaped text
//! | escaped_text     |
//! +------------------+
//! ```
//!
//! Escape byte 255 means "next byte is a literal" (escape-escape).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// FSST codec id (allocated in omnizip-codecs).

/// The escape-escape byte. When this appears in the output, the next
/// byte is a literal (not a symbol lookup).
const ESCAPE_ESCAPE: u8 = 255;

/// Maximum number of symbols in the table (0–254; byte 255 is the
/// escape-escape).
const MAX_SYMBOLS: usize = 255;

/// Maximum symbol length.
#[allow(dead_code)]
const MAX_SYMBOL_LEN: usize = 8;

/// A trained FSST symbol table.
#[derive(Clone, Debug)]
pub struct SymbolTable {
    /// `symbols[i]` = the i-th symbol's bytes. Index 0..n_symbols.
    symbols: Vec<Vec<u8>>,
}

impl SymbolTable {
    /// Build a symbol table from `input` by counting substring
    /// frequencies and greedily selecting the highest-gain symbols.
    ///
    /// Uses a hash-based approach: for each position, extract all
    /// 1–8 byte substrings, count them, then select greedily while
    /// accounting for overlaps (a chosen symbol "consumes" its
    /// occurrences).
    #[must_use]
    pub fn train(input: &[u8]) -> Self {
        if input.is_empty() {
            return Self {
                symbols: Vec::new(),
            };
        }

        // Phase 1: Count all 1–8 byte substrings using a HashMap.
        // To keep memory bounded, only count substrings up to length 8
        // and sample positions if input is very large.
        let mut counts: std::collections::HashMap<Vec<u8>, u32> = std::collections::HashMap::new();
        let step = if input.len() > 1_000_000 { 4 } else { 1 };

        for &len in &[1, 2, 3, 4, 5, 6, 7, 8] {
            let mut pos = 0;
            while pos + len <= input.len() {
                let sub = &input[pos..pos + len];
                *counts.entry(sub.to_vec()).or_insert(0) += 1;
                pos += step;
            }
        }

        // Phase 2: Greedy selection. Compute gain = (len-1) × count,
        // sort descending, and select non-overlapping symbols.
        let mut candidates: Vec<(Vec<u8>, u32)> = counts
            .iter()
            .filter(|(k, _)| k.len() >= 2) // Single-byte symbols give zero gain
            .map(|(k, &v)| (k.clone(), v * (k.len() as u32 - 1)))
            .filter(|(_, gain)| *gain > 0)
            .collect();
        candidates.sort_by(|a, b| {
            b.1.cmp(&a.1) // gain descending
                .then(a.0.len().cmp(&b.0.len())) // shorter first (faster to match)
                .then(a.0.cmp(&b.0)) // lexicographic byte order for determinism
        });

        let mut symbols: Vec<Vec<u8>> = Vec::new();
        let mut chosen: Vec<Vec<u8>> = Vec::new();
        for (sym, _gain) in candidates {
            if symbols.len() >= MAX_SYMBOLS {
                break;
            }
            // Skip if this symbol is a substring of an already-chosen
            // symbol (redundant). This is O(n×len) but n ≤ 255.
            let is_redundant = chosen
                .iter()
                .any(|c| c.windows(sym.len()).any(|w| w == sym.as_slice()));
            if is_redundant {
                continue;
            }
            symbols.push(sym.clone());
            chosen.push(sym);
        }

        Self { symbols }
    }

    /// Number of symbols in the table.
    #[must_use]
    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    /// Whether the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    /// Serialize the table to bytes.
    fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            1 + self.symbols.len() + self.symbols.iter().map(|s| s.len()).sum::<usize>(),
        );
        out.push(self.symbols.len() as u8);
        for s in &self.symbols {
            out.push(s.len() as u8);
        }
        for s in &self.symbols {
            out.extend_from_slice(s);
        }
        out
    }

    /// Deserialize a table from bytes. Returns (table, bytes consumed).
    fn deserialize(data: &[u8]) -> Option<(Self, usize)> {
        if data.is_empty() {
            return None;
        }
        let n = usize::from(data[0]);
        if data.len() < 1 + n {
            return None;
        }
        let lens: Vec<usize> = data[1..1 + n].iter().map(|&l| usize::from(l)).collect();
        let total_data: usize = lens.iter().sum();
        if data.len() < 1 + n + total_data {
            return None;
        }
        let mut symbols = Vec::with_capacity(n);
        let mut offset = 1 + n;
        for &len in &lens {
            symbols.push(data[offset..offset + len].to_vec());
            offset += len;
        }
        Some((Self { symbols }, offset))
    }
}

/// Compress `input` using FSST. Returns the serialized table + escaped
/// text.
///
/// # Errors
///
/// Returns [`OmnizipError::EncodeFailed`] only on internal errors.
pub fn compress(input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    let table = SymbolTable::train(input);
    let escaped = escape_text(input, &table);
    let mut out = table.serialize();
    out.extend_from_slice(&escaped);
    Ok(out)
}

/// Escape `input` using the symbol table. Performs longest-match
/// replacement.
fn escape_text(input: &[u8], table: &SymbolTable) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut pos = 0;
    while pos < input.len() {
        // Find the longest matching symbol at this position.
        let mut best: Option<u8> = None;
        let mut best_len = 0;
        for (i, sym) in table.symbols.iter().enumerate() {
            if pos + sym.len() <= input.len() && sym.len() > best_len {
                if &input[pos..pos + sym.len()] == sym.as_slice() {
                    best = Some(i as u8);
                    best_len = sym.len();
                }
            }
        }
        if let Some(sym_idx) = best {
            out.push(sym_idx);
            pos += best_len;
        } else {
            // No match: emit escape-escape + literal byte.
            out.push(ESCAPE_ESCAPE);
            out.push(input[pos]);
            pos += 1;
        }
    }
    out
}

/// Decompress FSST-compressed data. Expects the format from [`compress`].
///
/// # Errors
///
/// Returns [`OmnizipError::DecodeFailed`] on malformed input.
pub fn decompress(compressed: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    let (table, header_len) =
        SymbolTable::deserialize(compressed).ok_or(OmnizipError::DecodeFailed {
            codec: CodecId::FSST,
            reason: "malformed symbol table header".into(),
        })?;
    let escaped = &compressed[header_len..];
    let mut out = Vec::with_capacity(escaped.len() * 2);
    let mut pos = 0;
    while pos < escaped.len() {
        let b = escaped[pos];
        if b == ESCAPE_ESCAPE {
            pos += 1;
            if pos >= escaped.len() {
                return Err(OmnizipError::DecodeFailed {
                    codec: CodecId::FSST,
                    reason: "trailing escape-escape without literal".into(),
                });
            }
            out.push(escaped[pos]);
            pos += 1;
        } else {
            let idx = usize::from(b);
            if idx >= table.symbols.len() {
                return Err(OmnizipError::DecodeFailed {
                    codec: CodecId::FSST,
                    reason: format!("symbol index {idx} out of range"),
                });
            }
            out.extend_from_slice(&table.symbols[idx]);
            pos += 1;
        }
    }
    Ok(out)
}

/// FSST codec adapter implementing the omnizip-codecs `Codec` trait.
#[derive(Clone, Copy, Debug, Default)]
pub struct FsstCodec;

impl FsstCodec {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Codec for FsstCodec {
    fn id(&self) -> CodecId {
        CodecId::FSST
    }

    fn name(&self) -> &'static str {
        "fsst"
    }

    fn compress(
        &self,
        plaintext: &[u8],
        _level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        compress(plaintext)
    }

    fn decompress(&self, compressed: &[u8], _expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        decompress(compressed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_round_trips() {
        let compressed = compress(b"").expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert!(decompressed.is_empty());
    }

    #[test]
    fn short_input_round_trips() {
        let input = b"hello world";
        let compressed = compress(input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn repetitive_input_compresses() {
        // "the quick brown fox" repeated 100 times.
        let input: Vec<u8> = b"the quick brown fox ".repeat(100);
        let compressed = compress(&input).expect("compress");
        assert!(
            compressed.len() < input.len(),
            "FSST should compress repetitive text"
        );
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn random_input_round_trips() {
        // Random-ish data that FSST won't compress but must round-trip.
        let input: Vec<u8> = (0..1000).map(|i| ((i * 7919) % 256) as u8).collect();
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn determinism() {
        let input = b"the quick brown fox jumps over the lazy dog".repeat(10);
        let a = compress(&input).expect("compress");
        let b = compress(&input).expect("compress");
        assert_eq!(a, b, "FSST must be deterministic");
    }

    #[test]
    fn codec_trait_round_trips() {
        let codec = FsstCodec::new();
        let input = b"repetitive repetitive repetitive repetitive text".to_vec();
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .expect("compress");
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn csv_like_input_compresses_well() {
        // CSV-like data: common column headers repeated.
        let input = b"id,name,email,department,salary\n".repeat(200);
        let compressed = compress(&input).expect("compress");
        let ratio = compressed.len() as f64 / input.len() as f64;
        assert!(
            ratio < 0.3,
            "CSV should compress well with FSST, got {ratio:.2}"
        );
    }
}
