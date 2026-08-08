//! Pre-computed hash table for Brotli static dictionary lookups.
//!
//! Replaces the O(N) linear scan in [`find_dictionary_match`] with an
//! O(1) hash-table lookup. Supports three length-preserving transforms:
//! identity (0), UPPERCASE_FIRST (9), UPPERCASE_ALL (44).
//!
//! The table is built once via [`OnceLock`] and shared across all
//! encoder calls. Build cost: ~11K entries (3700 words × 3 transforms),
//! ~0.5 MB.

#![forbid(unsafe_code)]

use std::sync::OnceLock;

use crate::dictionary::{
    transform_dictionary_word, DICTIONARY_DATA, OFFSETS_BY_LENGTH, SIZE_BITS_BY_LENGTH,
};

const HASH_BITS: u32 = 16;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_PRIME: u32 = 2_654_435_761;

const TRANSFORM_IDENTITY: u8 = 0;
const TRANSFORM_UPPERCASE_FIRST: u8 = 9;
const TRANSFORM_UPPERCASE_ALL: u8 = 44;

const SUPPORTED_TRANSFORMS: [u8; 3] = [
    TRANSFORM_IDENTITY,
    TRANSFORM_UPPERCASE_FIRST,
    TRANSFORM_UPPERCASE_ALL,
];

#[derive(Clone, Copy)]
struct DictEntry {
    word_idx: u16,
    word_length: u8,
    transform_idx: u8,
}

struct DictHashTable {
    head: Vec<u32>,
    chain: Vec<u32>,
    entries: Vec<DictEntry>,
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
    let mut chain = Vec::new();
    let mut entries = Vec::new();

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

            for &transform_idx in &SUPPORTED_TRANSFORMS {
                let mut transformed = Vec::with_capacity(word_length + 8);
                let tlen =
                    transform_dictionary_word(&mut transformed, word, transform_idx as usize);
                if tlen < 4 || tlen != word_length {
                    continue;
                }
                let h = hash4(&transformed[..4]) as usize;
                let entry_idx = entries.len() as u32;
                entries.push(DictEntry {
                    word_idx: word_idx as u16,
                    word_length: word_length as u8,
                    transform_idx,
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
    }
}

/// Find the best dictionary match at `pos` using the pre-computed hash
/// table. Returns `(distance, copy_length)` or `None`.
///
/// Checks identity, UPPERCASE_FIRST, and UPPERCASE_ALL transforms.
/// `max_distance` should be `min(output_len, max_backward_distance)`.
#[must_use]
pub fn find_match(input: &[u8], pos: usize, max_distance: u32) -> Option<(u32, u32)> {
    if pos + 4 > input.len() {
        return None;
    }

    let table = get_table();
    let h = hash4(&input[pos..]) as usize;
    let mut idx = table.head[h];

    let mut best_len: u32 = 0;
    let mut best_distance: u32 = 0;

    while idx != u32::MAX {
        let entry = table.entries[idx as usize];
        let len = entry.word_length as usize;
        let shift = SIZE_BITS_BY_LENGTH[len];
        let offset_base = OFFSETS_BY_LENGTH[len] as usize;
        let dict_offset = offset_base + entry.word_idx as usize * len;

        if dict_offset + len <= DICTIONARY_DATA.len() && pos + len <= input.len() {
            let word = &DICTIONARY_DATA[dict_offset..dict_offset + len];
            if verify_transform(input, pos, word, entry.transform_idx) {
                if len as u32 > best_len {
                    best_len = len as u32;
                    let address = (entry.word_idx as u32) | ((entry.transform_idx as u32) << shift);
                    best_distance = max_distance + 1 + address;
                }
            }
        }

        idx = table.chain[idx as usize];
    }

    if best_len >= 4 {
        Some((best_distance, best_len))
    } else {
        None
    }
}

fn verify_transform(input: &[u8], pos: usize, word: &[u8], transform_idx: u8) -> bool {
    match transform_idx {
        TRANSFORM_IDENTITY => word == &input[pos..pos + word.len()],
        TRANSFORM_UPPERCASE_FIRST => verify_uppercase_first(input, pos, word),
        TRANSFORM_UPPERCASE_ALL => verify_uppercase_all(input, pos, word),
        _ => false,
    }
}

fn verify_uppercase_first(input: &[u8], pos: usize, word: &[u8]) -> bool {
    if word.is_empty() {
        return false;
    }
    let mut first = [word[0], 0, 0];
    let avail = word.len().min(3);
    first[..avail].copy_from_slice(&word[..avail]);
    let step = to_upper_inplace(&mut first, avail);
    if &input[pos..pos + step] != &first[..step] {
        return false;
    }
    &input[pos + step..pos + word.len()] == &word[step..]
}

fn verify_uppercase_all(input: &[u8], pos: usize, word: &[u8]) -> bool {
    let mut off = 0usize;
    while off < word.len() {
        let avail = (word.len() - off).min(3);
        let mut buf = [0u8; 3];
        buf[..avail].copy_from_slice(&word[off..off + avail]);
        let step = to_upper_inplace(&mut buf, avail);
        if &input[pos + off..pos + off + step] != &buf[..step] {
            return false;
        }
        off += step;
    }
    true
}

fn to_upper_inplace(buf: &mut [u8; 3], len: usize) -> usize {
    if buf[0] < 0xC0 {
        if (b'a'..=b'z').contains(&buf[0]) {
            buf[0] ^= 32;
        }
        return 1;
    }
    if buf[0] < 0xE0 {
        if len >= 2 {
            buf[1] ^= 32;
        }
        return 2;
    }
    if len >= 3 {
        buf[2] ^= 5;
    }
    3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_identity_dictionary_word() {
        // The first 4-byte dictionary word should match when present.
        let table = get_table();
        assert!(!table.entries.is_empty(), "hash table should have entries");

        // Use the first dictionary word's bytes as input.
        let word = &DICTIONARY_DATA[..4];
        let result = find_match(word, 0, 0);
        assert!(result.is_some(), "should find identity match");
        let (dist, len) = result.unwrap();
        assert_eq!(len, 4, "identity match length should be 4");
        assert!(dist > 0, "distance should be positive");
    }

    #[test]
    fn finds_uppercase_first_match() {
        // Find a lowercase ASCII dictionary word and check its
        // UPPERCASE_FIRST variant matches.
        let mut found_lower = false;
        for word_length in 4..=8usize {
            let shift = SIZE_BITS_BY_LENGTH[word_length];
            if shift == 0 {
                continue;
            }
            let num_words = 1usize << shift;
            let offset_base = OFFSETS_BY_LENGTH[word_length] as usize;
            for word_idx in 0..num_words.min(100) {
                let off = offset_base + word_idx * word_length;
                if off + word_length > DICTIONARY_DATA.len() {
                    break;
                }
                let word = &DICTIONARY_DATA[off..off + word_length];
                if (b'a'..=b'z').contains(&word[0]) {
                    let mut upper = word.to_vec();
                    upper[0] ^= 32;
                    let result = find_match(&upper, 0, 0);
                    assert!(
                        result.is_some(),
                        "UPPERCASE_FIRST should match for word starting with {:?}",
                        &word[..4]
                    );
                    found_lower = true;
                    break;
                }
            }
            if found_lower {
                break;
            }
        }
        assert!(found_lower, "should have found at least one lowercase word");
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
        // Should cover lengths 4..=24 (those with non-zero SIZE_BITS).
        for len in 4..=24usize {
            if SIZE_BITS_BY_LENGTH[len] > 0 {
                assert!(
                    lengths.contains(&(len as u8)),
                    "word length {len} should have entries"
                );
            }
        }
    }
}
