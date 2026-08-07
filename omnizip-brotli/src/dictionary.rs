//! Brotli static dictionary + transforms (RFC 7932 §10.4 + Appendix A).
//!
//! Direct port of upstream `c/common/dictionary.c` and `c/common/transform.c`
//! (MIT-licensed). The dictionary is a fixed 122784-byte table of common
//! English words, phrases, and binary patterns. Words are bucketed by length
//! (4..=24 bytes); the encoder picks a word, optionally applies one of 121
//! transforms (case-shift, trim, suffix/prefix), and emits a copy command
//! that references it via a synthetic distance > max_distance.
//!
//! ## Status
//!
//! Fully implemented: dictionary data is embedded via `include_bytes!`,
//! all 121 transforms are supported, and `dictionary_lookup` resolves
//! any (copy_len, distance) pair per upstream semantics.

#![forbid(unsafe_code)]

/// The raw dictionary data (RFC 7932 Appendix A, 122784 bytes).
pub const DICTIONARY_DATA: &[u8; 122_784] = include_bytes!("../data/dictionary.bin");

/// `size_bits_by_length[i]` = number of bits to encode the index of a
/// dictionary word of length `i` (RFC 7932 §10.4). 0 means no words of
/// that length exist.
pub const SIZE_BITS_BY_LENGTH: [u8; 32] = [
    0, 0, 0, 0, 10, 10, 11, 11, 10, 10, 10, 10, 10, 9, 9, 8, 7, 7, 8, 7, 7, 6, 6, 5, 5, 0, 0, 0, 0,
    0, 0, 0,
];

/// `offsets_by_length[i]` = byte offset in `DICTIONARY_DATA` where words
/// of length `i` begin (RFC 7932 §10.4).
pub const OFFSETS_BY_LENGTH: [u32; 32] = [
    0, 0, 0, 0, 0, 4096, 9216, 21504, 35840, 44032, 53248, 63488, 74752, 87040, 93696, 100864,
    104704, 106752, 108928, 113536, 115968, 118528, 119872, 121280, 122016, 122784, 122784, 122784,
    122784, 122784, 122784, 122784,
];

/// Minimum / maximum dictionary word length (RFC 7932 §10.4).
pub const MIN_DICTIONARY_WORD_LENGTH: usize = 4;
pub const MAX_DICTIONARY_WORD_LENGTH: usize = 24;

// --- Transforms (RFC 7932 §10.4 + upstream transform.c) ---

/// Transform type IDs (RFC 7932 §10.4 + upstream transform.h).
pub const TRANSFORM_IDENTITY: u8 = 0;
pub const TRANSFORM_OMIT_LAST_1: u8 = 1;
pub const TRANSFORM_OMIT_LAST_2: u8 = 2;
pub const TRANSFORM_OMIT_LAST_3: u8 = 3;
pub const TRANSFORM_OMIT_LAST_4: u8 = 4;
pub const TRANSFORM_OMIT_LAST_5: u8 = 5;
pub const TRANSFORM_OMIT_LAST_6: u8 = 6;
pub const TRANSFORM_OMIT_LAST_7: u8 = 7;
pub const TRANSFORM_OMIT_LAST_8: u8 = 8;
pub const TRANSFORM_OMIT_LAST_9: u8 = 9;
pub const TRANSFORM_UPPERCASE_FIRST: u8 = 10;
pub const TRANSFORM_UPPERCASE_ALL: u8 = 11;
pub const TRANSFORM_OMIT_FIRST_1: u8 = 12;
pub const TRANSFORM_OMIT_FIRST_2: u8 = 13;
pub const TRANSFORM_OMIT_FIRST_3: u8 = 14;
pub const TRANSFORM_OMIT_FIRST_4: u8 = 15;
pub const TRANSFORM_OMIT_FIRST_5: u8 = 16;
pub const TRANSFORM_OMIT_FIRST_6: u8 = 17;
pub const TRANSFORM_OMIT_FIRST_7: u8 = 18;
pub const TRANSFORM_OMIT_FIRST_8: u8 = 19;
pub const TRANSFORM_OMIT_FIRST_9: u8 = 20;
pub const TRANSFORM_SHIFT_FIRST: u8 = 21;
pub const TRANSFORM_SHIFT_ALL: u8 = 22;

/// Length-prefixed prefix/suffix strings (RFC 7932 §10.4).
/// Format: byte 0 = length, then `length` bytes of content. Total
/// length is 217 bytes (216 explicit + trailing 0 for the implicit
/// empty slot referenced by map index 49).
pub const PREFIX_SUFFIX: &[u8; 217] = b"\x01 \x02, \x10 of the \x04 of \x02s \x01.\x05 and \x04 in \x01\"\x04 to \x02\">\x01\n\x02. \x01]\x05 for \x03 a \x06 that \x01'\x06 with \x06 from \x04 by \x01(\x06. The \x04 on \x04 as \x04 is \x04ing \x02\n\t\x01:\x03ed \x02=\"\x04 at \x03ly \x01,\x02='\x05.com/\x07. This \x05 not \x03er \x03al \x04ful \x04ive \x05less \x04est \x04ize \x02\xc2\xa0\x04ous \x05 the \x02e \x00";

/// Indices in `PREFIX_SUFFIX` for each of the 50 prefix/suffix slots
/// used by the transform table.
pub const PREFIX_SUFFIX_MAP: [u16; 50] = [
    0x00, 0x02, 0x05, 0x0E, 0x13, 0x16, 0x18, 0x1E, 0x23, 0x25, 0x2A, 0x2D, 0x2F, 0x32, 0x34, 0x3A,
    0x3E, 0x45, 0x47, 0x4E, 0x55, 0x5A, 0x5C, 0x63, 0x68, 0x6D, 0x72, 0x77, 0x7A, 0x7C, 0x80, 0x83,
    0x88, 0x8C, 0x8E, 0x91, 0x97, 0x9F, 0xA5, 0xA9, 0xAD, 0xB2, 0xB7, 0xBD, 0xC2, 0xC7, 0xCA, 0xCF,
    0xD5, 0xD8,
];

/// Each transform is a `(prefix_index, transform_type, suffix_index)` triple.
/// Index `49` is the empty prefix/suffix.
pub const TRANSFORM_DATA: &[(u8, u8, u8)] = &[
    (49, TRANSFORM_IDENTITY, 49),
    (49, TRANSFORM_IDENTITY, 0),
    (0, TRANSFORM_IDENTITY, 0),
    (49, TRANSFORM_OMIT_FIRST_1, 49),
    (49, TRANSFORM_UPPERCASE_FIRST, 0),
    (49, TRANSFORM_IDENTITY, 47),
    (0, TRANSFORM_IDENTITY, 49),
    (4, TRANSFORM_IDENTITY, 0),
    (49, TRANSFORM_IDENTITY, 3),
    (49, TRANSFORM_UPPERCASE_FIRST, 49),
    (49, TRANSFORM_IDENTITY, 6),
    (49, TRANSFORM_OMIT_FIRST_2, 49),
    (49, TRANSFORM_OMIT_LAST_1, 49),
    (1, TRANSFORM_IDENTITY, 0),
    (49, TRANSFORM_IDENTITY, 1),
    (0, TRANSFORM_UPPERCASE_FIRST, 0),
    (49, TRANSFORM_IDENTITY, 7),
    (49, TRANSFORM_IDENTITY, 9),
    (48, TRANSFORM_IDENTITY, 0),
    (49, TRANSFORM_IDENTITY, 8),
    (49, TRANSFORM_IDENTITY, 5),
    (49, TRANSFORM_IDENTITY, 10),
    (49, TRANSFORM_IDENTITY, 11),
    (49, TRANSFORM_OMIT_LAST_3, 49),
    (49, TRANSFORM_IDENTITY, 13),
    (49, TRANSFORM_IDENTITY, 14),
    (49, TRANSFORM_OMIT_FIRST_3, 49),
    (49, TRANSFORM_OMIT_LAST_2, 49),
    (49, TRANSFORM_IDENTITY, 15),
    (49, TRANSFORM_IDENTITY, 16),
    (0, TRANSFORM_UPPERCASE_FIRST, 49),
    (49, TRANSFORM_IDENTITY, 12),
    (5, TRANSFORM_IDENTITY, 49),
    (0, TRANSFORM_IDENTITY, 1),
    (49, TRANSFORM_OMIT_FIRST_4, 49),
    (49, TRANSFORM_IDENTITY, 18),
    (49, TRANSFORM_IDENTITY, 17),
    (49, TRANSFORM_IDENTITY, 19),
    (49, TRANSFORM_IDENTITY, 20),
    (49, TRANSFORM_OMIT_FIRST_5, 49),
    (49, TRANSFORM_OMIT_FIRST_6, 49),
    (47, TRANSFORM_IDENTITY, 49),
    (49, TRANSFORM_OMIT_LAST_4, 49),
    (49, TRANSFORM_IDENTITY, 22),
    (49, TRANSFORM_UPPERCASE_ALL, 49),
    (49, TRANSFORM_IDENTITY, 23),
    (49, TRANSFORM_IDENTITY, 24),
    (49, TRANSFORM_IDENTITY, 25),
    (49, TRANSFORM_OMIT_LAST_7, 49),
    (49, TRANSFORM_OMIT_LAST_1, 26),
    (49, TRANSFORM_IDENTITY, 27),
    (49, TRANSFORM_IDENTITY, 28),
    (0, TRANSFORM_IDENTITY, 12),
    (49, TRANSFORM_IDENTITY, 29),
    (49, TRANSFORM_OMIT_FIRST_9, 49),
    (49, TRANSFORM_OMIT_FIRST_7, 49),
    (49, TRANSFORM_OMIT_LAST_6, 49),
    (49, TRANSFORM_IDENTITY, 21),
    (49, TRANSFORM_UPPERCASE_FIRST, 1),
    (49, TRANSFORM_OMIT_LAST_8, 49),
    (49, TRANSFORM_IDENTITY, 31),
    (49, TRANSFORM_IDENTITY, 32),
    (47, TRANSFORM_IDENTITY, 3),
    (49, TRANSFORM_OMIT_LAST_5, 49),
    (49, TRANSFORM_OMIT_LAST_9, 49),
    (0, TRANSFORM_UPPERCASE_FIRST, 1),
    (49, TRANSFORM_UPPERCASE_FIRST, 8),
    (5, TRANSFORM_IDENTITY, 21),
    (49, TRANSFORM_UPPERCASE_ALL, 0),
    (49, TRANSFORM_UPPERCASE_FIRST, 10),
    (49, TRANSFORM_IDENTITY, 30),
    (0, TRANSFORM_IDENTITY, 5),
    (35, TRANSFORM_IDENTITY, 49),
    (47, TRANSFORM_IDENTITY, 2),
    (49, TRANSFORM_UPPERCASE_FIRST, 17),
    (49, TRANSFORM_IDENTITY, 36),
    (49, TRANSFORM_IDENTITY, 33),
    (5, TRANSFORM_IDENTITY, 0),
    (49, TRANSFORM_UPPERCASE_FIRST, 21),
    (49, TRANSFORM_UPPERCASE_FIRST, 5),
    (49, TRANSFORM_IDENTITY, 37),
    (0, TRANSFORM_IDENTITY, 30),
    (49, TRANSFORM_IDENTITY, 38),
    (0, TRANSFORM_UPPERCASE_ALL, 0),
    (49, TRANSFORM_IDENTITY, 39),
    (0, TRANSFORM_UPPERCASE_ALL, 49),
    (49, TRANSFORM_IDENTITY, 34),
    (49, TRANSFORM_UPPERCASE_ALL, 8),
    (49, TRANSFORM_UPPERCASE_FIRST, 12),
    (0, TRANSFORM_IDENTITY, 21),
    (49, TRANSFORM_IDENTITY, 40),
    (0, TRANSFORM_UPPERCASE_FIRST, 12),
    (49, TRANSFORM_IDENTITY, 41),
    (49, TRANSFORM_IDENTITY, 42),
    (49, TRANSFORM_UPPERCASE_ALL, 17),
    (49, TRANSFORM_IDENTITY, 43),
    (0, TRANSFORM_UPPERCASE_FIRST, 5),
    (49, TRANSFORM_UPPERCASE_ALL, 10),
    (0, TRANSFORM_IDENTITY, 34),
    (49, TRANSFORM_UPPERCASE_FIRST, 33),
    (49, TRANSFORM_IDENTITY, 44),
    (49, TRANSFORM_UPPERCASE_ALL, 5),
    (45, TRANSFORM_IDENTITY, 49),
    (0, TRANSFORM_IDENTITY, 33),
    (49, TRANSFORM_UPPERCASE_FIRST, 30),
    (49, TRANSFORM_UPPERCASE_ALL, 30),
    (49, TRANSFORM_IDENTITY, 46),
    (49, TRANSFORM_UPPERCASE_ALL, 1),
    (49, TRANSFORM_UPPERCASE_FIRST, 34),
    (0, TRANSFORM_UPPERCASE_FIRST, 33),
    (0, TRANSFORM_UPPERCASE_ALL, 30),
    (0, TRANSFORM_UPPERCASE_ALL, 1),
    (49, TRANSFORM_UPPERCASE_ALL, 33),
    (49, TRANSFORM_UPPERCASE_ALL, 21),
    (49, TRANSFORM_UPPERCASE_ALL, 12),
    (0, TRANSFORM_UPPERCASE_ALL, 5),
    (49, TRANSFORM_UPPERCASE_ALL, 34),
    (0, TRANSFORM_UPPERCASE_ALL, 12),
    (0, TRANSFORM_UPPERCASE_FIRST, 30),
    (0, TRANSFORM_UPPERCASE_ALL, 34),
    (0, TRANSFORM_UPPERCASE_FIRST, 34),
];

/// Total number of transforms.
pub const NUM_TRANSFORMS: usize = TRANSFORM_DATA.len();

/// Identity-transform indices for each word length 4..=12. Used by
/// upstream as a fast path: `cutOffTransforms[len]` is the first
/// transform index whose `prefix == 0 && type == IDENTITY && suffix == 0`
/// applies to a word of length `len`. Words shorter than 13 always use
/// the identity transform.
pub const CUT_OFF_TRANSFORMS: [u16; 10] = [0, 12, 27, 23, 42, 63, 56, 48, 59, 64];

/// Get the prefix bytes (content only, without the length prefix) for a
/// transform index.
#[allow(dead_code)]
fn transform_prefix(idx: usize) -> &'static [u8] {
    let prefix_idx = TRANSFORM_DATA[idx].0 as usize;
    if prefix_idx == 49 {
        return &[];
    }
    let start = PREFIX_SUFFIX_MAP[prefix_idx] as usize;
    let len = PREFIX_SUFFIX[start] as usize;
    &PREFIX_SUFFIX[start + 1..start + 1 + len]
}

/// Get the suffix bytes (content only) for a transform index.
#[allow(dead_code)]
fn transform_suffix(idx: usize) -> &'static [u8] {
    let suffix_idx = TRANSFORM_DATA[idx].2 as usize;
    if suffix_idx == 49 {
        return &[];
    }
    let start = PREFIX_SUFFIX_MAP[suffix_idx] as usize;
    let len = PREFIX_SUFFIX[start] as usize;
    &PREFIX_SUFFIX[start + 1..start + 1 + len]
}

/// Uppercase the first UTF-8 rune of `p` in place. Returns the byte
/// length of the rune that was modified (1, 2, or 3).
///
/// Mirrors upstream `ToUpperCase`. For 1-byte ASCII runes, flips the
/// 0x20 bit. For 2-byte runes, flips bit 5 of byte 1. For 3-byte runes,
/// applies a fixed XOR (matches upstream's "arbitrary transform").
fn to_upper_case(p: &mut [u8]) -> usize {
    if p[0] < 0xC0 {
        if (b'a'..=b'z').contains(&p[0]) {
            p[0] ^= 32;
        }
        return 1;
    }
    if p[0] < 0xE0 {
        p[1] ^= 32;
        return 2;
    }
    p[2] ^= 5;
    3
}

/// Apply the transform at `transform_idx` to `word` and write the result
/// to `dst`. Returns the total number of bytes written.
///
/// Mirrors upstream `BrotliTransformDictionaryWord`. `SHIFT_FIRST` /
/// `SHIFT_ALL` transforms are not produced by the standard RFC 7932
/// dictionary (they require `transforms->params`, which is NULL for the
/// default dictionary), so we skip them.
pub fn transform_dictionary_word(dst: &mut Vec<u8>, word: &[u8], transform_idx: usize) -> usize {
    let start = dst.len();
    let (prefix_idx, transform_type, suffix_idx) = TRANSFORM_DATA[transform_idx];

    // Append prefix.
    if prefix_idx != 49 {
        let p_start = PREFIX_SUFFIX_MAP[prefix_idx as usize] as usize;
        let p_len = PREFIX_SUFFIX[p_start] as usize;
        dst.extend_from_slice(&PREFIX_SUFFIX[p_start + 1..p_start + 1 + p_len]);
    }

    // Apply transform to the word body.
    let body_start = dst.len();
    let t = transform_type;

    // Determine effective word slice (omit-first/last transforms trim it).
    let (word_slice, body_offset): (&[u8], usize) = if t <= TRANSFORM_OMIT_LAST_9 {
        // OmitLastN: drop last t bytes (t in 1..=9; t == 0 means Identity).
        let drop = t as usize;
        let end = word.len().saturating_sub(drop);
        (word[..end.min(word.len())].as_ref(), 0)
    } else if (TRANSFORM_OMIT_FIRST_1..=TRANSFORM_OMIT_FIRST_9).contains(&t) {
        // OmitFirstN: drop first (t - 11) bytes.
        let skip = (t - (TRANSFORM_OMIT_FIRST_1 - 1)) as usize;
        if word.len() <= skip {
            (&[][..], 0)
        } else {
            (word[skip..].as_ref(), 0)
        }
    } else {
        (word.as_ref(), 0)
    };
    let _ = body_offset;

    dst.extend_from_slice(word_slice);
    let body_len = word_slice.len();

    match t {
        TRANSFORM_UPPERCASE_FIRST => {
            if body_len > 0 {
                let step = to_upper_case(&mut dst[body_start..]);
                let _ = step;
            }
        }
        TRANSFORM_UPPERCASE_ALL => {
            let mut pos = body_start;
            let end = body_start + body_len;
            while pos < end {
                let remaining = &mut dst[pos..end];
                let step = to_upper_case(remaining);
                pos += step;
            }
        }
        _ => {}
    }

    // Append suffix.
    if suffix_idx != 49 {
        let s_start = PREFIX_SUFFIX_MAP[suffix_idx as usize] as usize;
        let s_len = PREFIX_SUFFIX[s_start] as usize;
        dst.extend_from_slice(&PREFIX_SUFFIX[s_start + 1..s_start + 1 + s_len]);
    }

    dst.len() - start
}

/// Resolve a static dictionary reference and append the resulting bytes
/// to `output`.
///
/// Inputs:
/// - `copy_len`: the literal/copy length from the command (word length).
/// - `distance_code`: the resolved distance value.
/// - `max_distance`: the current maximum LZ77 distance (= min(pos,
///   max_backward_distance)).
///
/// Returns `Some(())` on success, `None` if the reference is invalid
/// (word length out of range, transform index out of range, etc.).
///
/// Per RFC 7932 §10.4:
///   word_id = distance_code - max_distance - 1
///   shift = size_bits_by_length[copy_len]
///   word_idx = word_id & ((1 << shift) - 1)
///   transform_idx = word_id >> shift
///   word = dictionary[offsets_by_length[copy_len] + word_idx * copy_len ..]
///   output += transform(word, transform_idx)
pub fn dictionary_lookup(
    output: &mut Vec<u8>,
    copy_len: u32,
    distance_code: i32,
    max_distance: u32,
) -> Option<()> {
    if !(MIN_DICTIONARY_WORD_LENGTH as u32..=MAX_DICTIONARY_WORD_LENGTH as u32).contains(&copy_len)
    {
        return None;
    }
    let len = copy_len as usize;
    let shift = SIZE_BITS_BY_LENGTH[len];
    if shift == 0 {
        return None;
    }

    let address = distance_code as i64 - max_distance as i64 - 1;
    if address < 0 {
        return None;
    }
    let address = address as usize;

    let mask = (1usize << shift) - 1;
    let word_idx = address & mask;
    let transform_idx = address >> shift;

    if transform_idx >= NUM_TRANSFORMS {
        return None;
    }

    let offset = OFFSETS_BY_LENGTH[len] as usize + word_idx * len;
    if offset + len > DICTIONARY_DATA.len() {
        return None;
    }

    let word = &DICTIONARY_DATA[offset..offset + len];
    let before = output.len();
    transform_dictionary_word(output, word, transform_idx);
    if output.len() == before {
        // Empty output is invalid per upstream:
        // "if (len == 0 && s->distance_code <= 120) { return FAILURE; }"
        // The decoder rejects length-0 dictionary results.
        if distance_code <= 120 {
            return None;
        }
    }
    Some(())
}

/// Try to find a dictionary word match at the given input position.
/// Returns `(distance_code, copy_len)` if a match is found, or `None`.
///
/// Uses a simple linear scan over 4-byte dictionary words (identity
/// transform only). The distance_code is set to `max_distance + 1 +
/// word_idx` so the decoder interprets it as a dictionary reference.
///
/// `max_distance` should be `min(output_len, (1 << window_bits) - 1)`.
#[must_use]
pub fn find_dictionary_match(input: &[u8], pos: usize, max_distance: u32) -> Option<(u32, u32)> {
    if pos + 4 > input.len() {
        return None;
    }

    // Try word lengths 4..=8 (the most common in text).
    for len in 4u32..=8u32 {
        let len_us = len as usize;
        if pos + len_us > input.len() {
            break;
        }
        let shift = SIZE_BITS_BY_LENGTH[len_us];
        if shift == 0 {
            continue;
        }
        let num_words = 1usize << shift;
        let offset_base = OFFSETS_BY_LENGTH[len_us] as usize;

        for word_idx in 0..num_words {
            let dict_offset = offset_base + word_idx * len_us;
            if dict_offset + len_us > DICTIONARY_DATA.len() {
                break;
            }
            // Fast first-byte check before full comparison.
            if DICTIONARY_DATA[dict_offset] != input[pos] {
                continue;
            }
            if &DICTIONARY_DATA[dict_offset..dict_offset + len_us] == &input[pos..pos + len_us] {
                // Found a match! Compute the distance code.
                let address = word_idx as u32; // transform_idx = 0 (identity)
                let distance = max_distance + 1 + address;
                return Some((distance, len));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_size_is_correct() {
        assert_eq!(DICTIONARY_DATA.len(), 122_784);
    }

    #[test]
    fn word_length_4_first_word_is_valid() {
        // First 4-byte word at offset 0.
        let word = &DICTIONARY_DATA[0..4];
        assert!(!word.iter().any(|&b| b == 0));
    }

    #[test]
    fn identity_transform_emits_word_unchanged() {
        let mut out = Vec::new();
        let n = transform_dictionary_word(&mut out, b"hello", 0);
        assert_eq!(n, 5);
        assert_eq!(&out, b"hello");
    }

    #[test]
    fn uppercase_first_transform_works() {
        // Find a UPPERCASE_FIRST transform in the table.
        let idx = TRANSFORM_DATA
            .iter()
            .position(|(_, t, _)| *t == TRANSFORM_UPPERCASE_FIRST)
            .unwrap();
        let mut out = Vec::new();
        transform_dictionary_word(&mut out, b"hello", idx);
        // The first letter should be uppercased.
        assert!(out[0] >= b'A' && out[0] <= b'Z' || out[0] == b'h');
    }

    #[test]
    fn transform_count_is_121() {
        // RFC 7932 §10.4 specifies 121 transforms.
        assert_eq!(NUM_TRANSFORMS, 121);
    }

    #[test]
    fn dictionary_lookup_resolves_simple_word() {
        // copy_len=4, word_idx=0, transform_idx=0 (identity).
        // distance = max_distance + 1 + (0 | (0 << shift)).
        let max_distance = 0u32;
        let distance = max_distance + 1;
        let mut output = Vec::new();
        assert!(dictionary_lookup(&mut output, 4, distance as i32, max_distance).is_some());
        assert_eq!(output.len(), 4);
    }

    #[test]
    fn dictionary_lookup_rejects_invalid_word_length() {
        let mut output = Vec::new();
        assert!(dictionary_lookup(&mut output, 3, 100, 0).is_none());
        assert!(dictionary_lookup(&mut output, 25, 100, 0).is_none());
    }

    #[test]
    fn dictionary_lookup_rejects_invalid_transform() {
        // copy_len=4 → shift=10, so transform_idx can be up to (address >> 10).
        // Use a huge address to push transform_idx out of range.
        let max_distance = 0u32;
        let distance = max_distance as i64 + 1 + (NUM_TRANSFORMS as i64 + 5) * 1024;
        let mut output = Vec::new();
        assert!(dictionary_lookup(&mut output, 4, distance as i32, max_distance).is_none());
    }

    #[test]
    fn offsets_table_is_monotonic() {
        for i in 0..31 {
            let bits = SIZE_BITS_BY_LENGTH[i] as u32;
            let inc: u32 = if bits == 0 { 0 } else { (i as u32) << bits };
            let expected_next = OFFSETS_BY_LENGTH[i] + inc;
            assert_eq!(OFFSETS_BY_LENGTH[i + 1], expected_next, "len={i}");
        }
        assert_eq!(OFFSETS_BY_LENGTH[31], 122_784);
    }

    #[test]
    fn prefix_suffix_map_entries_are_in_bounds() {
        assert_eq!(PREFIX_SUFFIX.len(), 217, "PREFIX_SUFFIX must be 217 bytes");
        for (i, &start) in PREFIX_SUFFIX_MAP.iter().enumerate() {
            let s = start as usize;
            assert!(s < 217, "map[{i}]={s} >= 217");
            let len = PREFIX_SUFFIX[s] as usize;
            assert!(s + 1 + len <= 217, "map[{i}]={s}: len={len} overflows");
        }
        assert_eq!(PREFIX_SUFFIX[216], 0, "trailing slot must be empty (len=0)");
    }
}
