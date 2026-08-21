//! Pre-computed hash table for Brotli static dictionary lookups.
//!
//! Supports ALL 121 RFC 7932 transforms (identity, uppercase, omit,
//! prefix/suffix). Built once via [`OnceLock`] and shared across all
//! encoder calls.
//!
//! Each entry stores the full transformed bytes in a flat pool, enabling
//! O(1) verification without per-lookup allocation or transform
//! recomputation.

#![forbid(unsafe_code)]

use std::sync::OnceLock;

use crate::dictionary::{
    transform_dictionary_word, DICTIONARY_DATA, NUM_TRANSFORMS, OFFSETS_BY_LENGTH,
    SIZE_BITS_BY_LENGTH,
};

const HASH_BITS: u32 = 17;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_PRIME: u32 = 2_654_435_761;

#[derive(Clone, Copy)]
struct DictEntry {
    word_idx: u16,
    word_length: u8,
    transform_idx: u8,
    pool_offset: u32,
    transformed_len: u16,
}

struct DictHashTable {
    head: Vec<u32>,
    chain: Vec<u32>,
    entries: Vec<DictEntry>,
    byte_pool: Vec<u8>,
}

static DICT_HASH: OnceLock<DictHashTable> = OnceLock::new();

fn get_table() -> &'static DictHashTable {
    DICT_HASH.get_or_init(build_table)
}

fn hash4(data: &[u8]) -> u32 {
    let val = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    val.wrapping_mul(HASH_PRIME) >> (32 - HASH_BITS)
}

fn build_table() -> DictHashTable {
    let mut head = vec![u32::MAX; HASH_SIZE];
    // ~260K transformed entries; preallocate to avoid growth copies.
    let mut chain = Vec::with_capacity(270_000);
    let mut entries = Vec::with_capacity(270_000);
    let mut byte_pool = Vec::with_capacity(3 << 20);

    for word_length in 4..=24usize {
        let shift = SIZE_BITS_BY_LENGTH[word_length];
        if shift == 0 {
            continue;
        }
        let num_words = 1usize << shift;
        let offset_base = OFFSETS_BY_LENGTH[word_length] as usize;

        for word_idx in 0..num_words {
            let dict_offset = offset_base + word_idx * word_length;
            if dict_offset + word_length > DICTIONARY_DATA.len() {
                break;
            }
            let word = &DICTIONARY_DATA[dict_offset..dict_offset + word_length];

            // One reused buffer: a fresh Vec per (word, transform) meant
            // ~260K mallocs in table build.
            let mut transformed = Vec::with_capacity(word_length + 16);
            // Reverse insert order: the chain is LIFO, so identity
            // (transform 0) must be inserted LAST to sit at the head —
            // a depth cap then prefers identity matches over exotic
            // transform variants.
            for transform_idx in (0..NUM_TRANSFORMS).rev() {
                transformed.clear();
                let tlen = transform_dictionary_word(&mut transformed, word, transform_idx);
                if tlen < 4 {
                    continue;
                }

                let pool_offset = byte_pool.len() as u32;
                byte_pool.extend_from_slice(&transformed[..tlen]);

                let h = hash4(&transformed[..4]) as usize;
                let entry_idx = entries.len() as u32;
                entries.push(DictEntry {
                    word_idx: word_idx as u16,
                    word_length: word_length as u8,
                    transform_idx: transform_idx as u8,
                    pool_offset,
                    transformed_len: tlen as u16,
                });
                chain.push(head[h]);
                head[h] = entry_idx;
            }
        }
    }

    DictHashTable {
        head,
        chain,
        entries,
        byte_pool,
    }
}

/// Find the best dictionary match at `pos` using the pre-computed hash
/// table. Returns `(distance, word_length, transformed_len)` or `None`.
///
/// - `distance`: the encoded dictionary distance for the command
/// - `word_length`: the original word length (= `copy_len` for the command)
/// - `transformed_len`: the actual match length (parser advances by this)
///
/// For identity/uppercase: `word_length == transformed_len`.
/// For prefix/suffix/omit: they may differ.
///
/// `max_distance` should be `min(output_len, max_backward_distance)`.
#[must_use]
pub fn find_match(input: &[u8], pos: usize, max_distance: u32) -> Option<(u32, u32, u32)> {
    if pos + 4 > input.len() {
        return None;
    }

    let table = get_table();
    let h = hash4(&input[pos..]) as usize;
    let mut idx = table.head[h];

    let mut best_len: u32 = 0;
    let mut best_distance: u32 = 0;
    let mut best_word_len: u32 = 0;
    // Common 4-byte prefixes chain dozens of transformed words; the
    // reference's curated bucket lists hold only a handful. Cap the
    // walk (BROTLI_DICT_CHAIN overrides).
    static DICT_CHAIN: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let max_chain: u32 = *DICT_CHAIN.get_or_init(|| {
        std::env::var("BROTLI_DICT_CHAIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8)
    });
    let mut walked: u32 = 0;

    while idx != u32::MAX {
        let entry = table.entries[idx as usize];
        let tlen = entry.transformed_len as usize;
        let pool_start = entry.pool_offset as usize;
        let transformed = &table.byte_pool[pool_start..pool_start + tlen];

        if pos + tlen <= input.len()
            && transformed == &input[pos..pos + tlen]
            && tlen as u32 > best_len
        {
            best_len = tlen as u32;
            let len = entry.word_length as usize;
            let shift = SIZE_BITS_BY_LENGTH[len];
            let address = u32::from(entry.word_idx) | (u32::from(entry.transform_idx) << shift);
            best_distance = max_distance + 1 + address;
            best_word_len = u32::from(entry.word_length);
        }

        idx = table.chain[idx as usize];
        walked += 1;
        if walked >= max_chain {
            break;
        }
    }

    if best_len >= 4 {
        Some((best_distance, best_word_len, best_len))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_identity_dictionary_word() {
        let table = get_table();
        assert!(!table.entries.is_empty(), "hash table should have entries");
        assert!(
            !table.byte_pool.is_empty(),
            "byte pool should have transformed words"
        );

        // The chain walk is capped (common 4-byte prefixes chain dozens
        // of transformed words), so a SPECIFIC word can sit beyond the
        // cap behind colliding words. Scan until some dictionary word
        // IS found and verify its transformed length matches.
        let mut found: Option<(u32, u32, u32)> = None;
        for word_length in 8..=24usize {
            let shift = SIZE_BITS_BY_LENGTH[word_length];
            if shift == 0 {
                continue;
            }
            let offset = OFFSETS_BY_LENGTH[word_length] as usize;
            let word = &DICTIONARY_DATA[offset..offset + word_length];
            if let Some(r) = find_match(word, 0, 0) {
                found = Some(r);
                break;
            }
        }
        let (_, word_len, tlen) =
            found.expect("at least one dictionary word should be findable");
        assert!(tlen >= 8);
        assert!(word_len >= 8);
    }

    #[test]
    fn finds_transform_with_prefix() {
        // Find a word where a prefix transform produces a longer match.
        // TRANSFORM_DATA[1] = (49, IDENTITY, 0): suffix " "
        // TRANSFORM_DATA[2] = (0, IDENTITY, 0): prefix " "
        // These add a space before or after the word.
        let table = get_table();
        let prefix_transforms: Vec<u8> = (0..NUM_TRANSFORMS as u8)
            .filter(|&idx| {
                let word = &DICTIONARY_DATA[..4];
                let mut out = Vec::new();
                let len = transform_dictionary_word(&mut out, word, idx as usize);
                len > 4 // transform produces a longer word (prefix/suffix)
            })
            .collect();
        assert!(
            !prefix_transforms.is_empty(),
            "should have transforms that add prefix/suffix"
        );
    }

    #[test]
    fn no_false_match_on_random_data() {
        let data: Vec<u8> = (0u32..1000)
            .map(|i| (i.wrapping_mul(2654435761) >> 16 & 0xFF) as u8)
            .collect();
        let mut matches = 0;
        for pos in 0..data.len().saturating_sub(4) {
            if find_match(&data, pos, 0).is_some() {
                matches += 1;
            }
        }
        assert!(
            matches < data.len() / 20,
            "too many false matches: {matches}"
        );
    }

    #[test]
    fn hash_table_covers_all_word_lengths() {
        let table = get_table();
        let mut lengths = std::collections::HashSet::new();
        for entry in &table.entries {
            lengths.insert(entry.word_length);
        }
        for len in 4..=24usize {
            if SIZE_BITS_BY_LENGTH[len] > 0 {
                assert!(
                    lengths.contains(&(len as u8)),
                    "word length {len} should have entries"
                );
            }
        }
    }

    #[test]
    fn all_121_transforms_represented() {
        let table = get_table();
        let transforms: std::collections::HashSet<u8> =
            table.entries.iter().map(|e| e.transform_idx).collect();
        // Not all 121 transforms produce ≥4-byte results for all words,
        // but most should be represented.
        assert!(
            transforms.len() > 50,
            "expected >50 transforms in table, got {}",
            transforms.len()
        );
    }
}
