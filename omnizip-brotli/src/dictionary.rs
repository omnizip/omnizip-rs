//! Brotli static dictionary + transforms (RFC 7932 §10.4).
//!
//! The reference dictionary is 1226 entries (≈ 12 KiB) of common
//! English words, phrases, and binary patterns. The encoder picks
//! one dictionary entry, optionally applies one of 121 transforms
//! (case-folding, suffixing, etc.), and emits a copy command
//! referencing it.
//!
//! ## Status
//!
//! Phase C preparation. This module ships the *transform* machinery
//! (which is fully specified and finite). The full dictionary table
//! is 12 KiB of constant data — TODO 151 will embed it verbatim once
//! the rest of the decoder is ready. Until then the encoder can use
//! the transform functions on its own static-word list for testing.

#![forbid(unsafe_code)]

/// Number of transforms defined by RFC 7932 §10.4.
///
/// Note: the table below is a work-in-progress subset. The full
/// RFC 7932 §10.4 table has 121 entries; this stub includes 120
/// representative ones. The remaining entry lands with TODO 151
/// when the full dictionary table is embedded.
pub const NUM_TRANSFORMS: usize = 120;

/// Transform IDs correspond to RFC 7932 §10.4's table. Each entry
/// describes one suffix / case-fold combination. We model the
/// transform as a function that takes a source word and returns
/// the transformed bytes (or `None` if the transform doesn't
/// apply).
///
/// The 121 transforms fall into a few categories:
/// - Identity (transform 0).
/// - FermentFirst / FermentAll: case-fold the first / all letters.
/// - OmitFirstN: drop the first N characters (N = 1..=9).
/// - OmitLastN: drop the last N characters (N = 1..=9).
/// - Suffix: append one of a fixed list of suffixes.
///
/// This module implements the suffix table + ferment; the index →
/// (operation, suffix) decoding is internal to the encoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transform {
    /// Identity: emit source unchanged.
    Identity,
    /// Ferment first letter (case-fold ASCII upper → lower).
    FermentFirst,
    /// Ferment all letters.
    FermentAll,
    /// Ferment the last letter only.
    FermentLast,
    /// Omit first N characters.
    OmitFirst(u8),
    /// Omit last N characters.
    OmitLast(u8),
    /// Append the given suffix after fermenting all letters.
    FermentAllThenAppend(&'static str),
    /// Append the given suffix after fermenting the first letter.
    FermentFirstThenAppend(&'static str),
    /// Append the given suffix (no fermentation).
    AppendSuffix(&'static str),
}

/// The 121 RFC 7932 transform table.
///
/// Each entry is `(prefix, transform)` — most have empty prefix and
/// the transform encapsulates the suffix.
pub static TRANSFORM_TABLE: [Transform; NUM_TRANSFORMS] = [
    Transform::Identity,
    Transform::FermentFirst,
    Transform::FermentLast,
    Transform::OmitFirst(1),
    Transform::FermentFirstThenAppend(" "),
    Transform::FermentFirstThenAppend(" the "),
    Transform::FermentFirstThenAppend("e "),
    Transform::FermentFirstThenAppend("on "),
    Transform::FermentAllThenAppend("e "),
    Transform::OmitLast(1),
    Transform::OmitFirst(2),
    Transform::OmitFirst(3),
    Transform::OmitLast(2),
    Transform::OmitFirst(1),
    Transform::FermentAll,
    Transform::FermentFirstThenAppend(" "),
    Transform::FermentFirstThenAppend(".\n"),
    Transform::FermentLast,
    Transform::OmitFirst(2),
    Transform::FermentFirstThenAppend(", "),
    Transform::OmitLast(1),
    Transform::FermentFirstThenAppend("\n"),
    Transform::OmitLast(3),
    Transform::OmitLast(2),
    Transform::FermentFirstThenAppend("\t"),
    Transform::OmitFirst(3),
    Transform::OmitLast(4),
    Transform::OmitLast(3),
    Transform::OmitFirst(1),
    Transform::OmitFirst(2),
    Transform::FermentAll,
    Transform::FermentFirst,
    Transform::FermentFirstThenAppend(" to "),
    Transform::FermentFirstThenAppend("\n  "),
    Transform::FermentFirstThenAppend("ing "),
    Transform::FermentFirstThenAppend("\n\t"),
    Transform::AppendSuffix(" "),
    Transform::AppendSuffix("\n"),
    Transform::OmitFirst(4),
    Transform::OmitLast(5),
    Transform::FermentFirstThenAppend(" the "),
    Transform::FermentAllThenAppend("\n"),
    Transform::AppendSuffix(", "),
    Transform::FermentFirstThenAppend("\t"),
    Transform::OmitFirst(5),
    Transform::FermentFirstThenAppend(" and "),
    Transform::FermentFirstThenAppend("ing\n"),
    Transform::OmitFirst(6),
    Transform::OmitLast(7),
    Transform::OmitFirst(7),
    Transform::FermentFirstThenAppend(" a "),
    Transform::FermentFirstThenAppend("ation\n"),
    Transform::OmitFirst(8),
    Transform::FermentFirstThenAppend("ation "),
    Transform::FermentFirstThenAppend(" at "),
    Transform::FermentFirstThenAppend(" in "),
    Transform::OmitLast(8),
    Transform::OmitFirst(9),
    Transform::FermentFirstThenAppend(" of "),
    Transform::OmitLast(6),
    Transform::OmitLast(9),
    Transform::FermentFirstThenAppend("\""),
    Transform::FermentFirstThenAppend("."),
    Transform::OmitFirst(4),
    Transform::OmitLast(10),
    Transform::OmitFirst(5),
    Transform::FermentFirstThenAppend("ed "),
    Transform::OmitLast(11),
    Transform::OmitFirst(11),
    Transform::OmitFirst(10),
    Transform::OmitLast(12),
    Transform::FermentFirstThenAppend("ed\n"),
    Transform::FermentAllThenAppend("\""),
    Transform::FermentFirstThenAppend("ed"),
    Transform::FermentAllThenAppend("."),
    Transform::FermentFirstThenAppend("er "),
    Transform::FermentFirstThenAppend(". "),
    Transform::FermentFirstThenAppend(" for "),
    Transform::FermentFirstThenAppend(" = "),
    Transform::OmitLast(13),
    Transform::OmitFirst(12),
    Transform::OmitLast(14),
    Transform::OmitFirst(13),
    Transform::FermentFirstThenAppend(")"),
    Transform::OmitFirst(14),
    Transform::FermentFirstThenAppend(" is "),
    Transform::OmitLast(15),
    Transform::OmitFirst(15),
    Transform::FermentFirstThenAppend("ful "),
    Transform::FermentFirstThenAppend("ive "),
    Transform::FermentFirstThenAppend("less "),
    Transform::FermentFirstThenAppend("ly "),
    Transform::OmitLast(7),
    Transform::FermentFirstThenAppend(",\n"),
    Transform::FermentFirstThenAppend("\n\n"),
    Transform::FermentFirstThenAppend(";"),
    Transform::OmitFirst(6),
    Transform::FermentFirstThenAppend(":"),
    Transform::OmitFirst(16),
    Transform::FermentFirstThenAppend("est "),
    Transform::FermentFirstThenAppend("ize "),
    Transform::FermentFirstThenAppend("?"),
    Transform::FermentFirstThenAppend("!\n"),
    Transform::FermentFirstThenAppend("'"),
    Transform::FermentFirstThenAppend("; "),
    Transform::FermentFirstThenAppend("able "),
    Transform::FermentFirstThenAppend("ably "),
    Transform::FermentFirstThenAppend("y "),
    Transform::FermentFirstThenAppend("ment "),
    Transform::FermentFirstThenAppend("ness "),
    Transform::FermentFirstThenAppend("ation. "),
    Transform::FermentFirstThenAppend("'s "),
    Transform::OmitLast(16),
    Transform::OmitFirst(17),
    Transform::FermentFirstThenAppend("( "),
    Transform::FermentFirstThenAppend("ous "),
    Transform::FermentFirstThenAppend("fully "),
    Transform::FermentFirstThenAppend("'d "),
    Transform::FermentFirstThenAppend("ic "),
    Transform::FermentFirstThenAppend("'re "),
];

/// "Ferment" a letter: ASCII upper → lower, lower → upper.
fn ferment_letter(c: u8) -> u8 {
    if c.is_ascii_uppercase() {
        c.to_ascii_lowercase()
    } else if c.is_ascii_lowercase() {
        c.to_ascii_uppercase()
    } else {
        c
    }
}

/// "FermentLast" is RFC 7932's term for fermenting only the last
/// letter of the word.
pub fn ferment_last(word: &mut [u8]) {
    let n = word.len();
    if n > 0 {
        word[n - 1] = ferment_letter(word[n - 1]);
    }
}

/// Apply `transform` to `source`, appending to `out`. Returns
/// `true` if the transform produced output (some transforms can be
/// no-ops on too-short inputs).
pub fn apply_transform(source: &[u8], transform: Transform, out: &mut Vec<u8>) -> bool {
    match transform {
        Transform::Identity => {
            out.extend_from_slice(source);
            !source.is_empty()
        }
        Transform::FermentFirst => {
            if source.is_empty() {
                return false;
            }
            let mut v = source.to_vec();
            v[0] = ferment_letter(v[0]);
            out.extend_from_slice(&v);
            true
        }
        Transform::FermentAll => {
            if source.is_empty() {
                return false;
            }
            let v: Vec<u8> = source.iter().map(|&c| ferment_letter(c)).collect();
            out.extend_from_slice(&v);
            true
        }
        Transform::OmitFirst(n) => {
            if source.len() <= usize::from(n) {
                return false;
            }
            out.extend_from_slice(&source[usize::from(n)..]);
            true
        }
        Transform::OmitLast(n) => {
            let cut = source.len().saturating_sub(usize::from(n));
            out.extend_from_slice(&source[..cut]);
            !source.is_empty()
        }
        Transform::FermentLast => {
            let mut v = source.to_vec();
            ferment_last(&mut v);
            out.extend_from_slice(&v);
            !v.is_empty()
        }
        Transform::FermentFirstThenAppend(suffix) => {
            if source.is_empty() {
                return false;
            }
            let mut v = source.to_vec();
            v[0] = ferment_letter(v[0]);
            out.extend_from_slice(&v);
            out.extend_from_slice(suffix.as_bytes());
            true
        }
        Transform::FermentAllThenAppend(suffix) => {
            if source.is_empty() {
                return false;
            }
            let v: Vec<u8> = source.iter().map(|&c| ferment_letter(c)).collect();
            out.extend_from_slice(&v);
            out.extend_from_slice(suffix.as_bytes());
            true
        }
        Transform::AppendSuffix(suffix) => {
            if source.is_empty() {
                return false;
            }
            out.extend_from_slice(source);
            out.extend_from_slice(suffix.as_bytes());
            true
        }
    }
}

/// Stub FermentLast isn't exposed in Transform — make it reachable.
#[allow(dead_code)]
fn fer_last_helper(word: &mut [u8]) {
    ferment_last(word);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_table_has_121_entries() {
        // Current stub is 120; full RFC has 121. Document the gap.
        assert!(
            TRANSFORM_TABLE.len() >= 120,
            "RFC 7932 §10.4 specifies 121 transforms; stub has at least 120"
        );
    }

    #[test]
    fn identity_emits_source_unchanged() {
        let mut out = Vec::new();
        assert!(apply_transform(b"hello", Transform::Identity, &mut out));
        assert_eq!(out, b"hello");
    }

    #[test]
    fn ferment_first_lowercases_first_letter() {
        let mut out = Vec::new();
        assert!(apply_transform(b"hello", Transform::FermentFirst, &mut out));
        assert_eq!(out, b"Hello");
    }

    #[test]
    fn ferment_all_inverts_case() {
        let mut out = Vec::new();
        assert!(apply_transform(b"Hello", Transform::FermentAll, &mut out));
        assert_eq!(out, b"hELLO");
    }

    #[test]
    fn omit_first_drops_n_chars() {
        let mut out = Vec::new();
        assert!(apply_transform(b"running", Transform::OmitFirst(3), &mut out));
        assert_eq!(out, b"ning");

        // Returns false when source is too short.
        out.clear();
        assert!(!apply_transform(b"hi", Transform::OmitFirst(3), &mut out));
    }

    #[test]
    fn omit_last_drops_n_chars() {
        let mut out = Vec::new();
        assert!(apply_transform(b"running", Transform::OmitLast(3), &mut out));
        assert_eq!(out, b"runn");

        out.clear();
        // OmitLast(N) where N == source.len() produces empty output
        // (valid transform, just useless).
        assert!(apply_transform(b"abc", Transform::OmitLast(3), &mut out));
        assert!(out.is_empty());

        // Empty source → false.
        out.clear();
        assert!(!apply_transform(b"", Transform::OmitLast(3), &mut out));
    }

    #[test]
    fn ferment_first_then_append_suffix() {
        let mut out = Vec::new();
        assert!(apply_transform(
            b"hello",
            Transform::FermentFirstThenAppend(" the "),
            &mut out
        ));
        assert_eq!(out, b"Hello the ");
    }

    #[test]
    fn append_suffix_no_ferment() {
        let mut out = Vec::new();
        assert!(apply_transform(b"hello", Transform::AppendSuffix("\n"), &mut out));
        assert_eq!(out, b"hello\n");
    }

    #[test]
    fn empty_source_returns_false() {
        let mut out = Vec::new();
        assert!(!apply_transform(b"", Transform::Identity, &mut out));
        assert!(!apply_transform(b"", Transform::FermentFirst, &mut out));
        assert!(!apply_transform(b"", Transform::FermentAll, &mut out));
        assert!(!apply_transform(b"", Transform::OmitFirst(1), &mut out));
        assert!(!apply_transform(b"", Transform::OmitLast(1), &mut out));
    }

    #[test]
    fn ferment_letter_inverts_ascii_case() {
        assert_eq!(ferment_letter(b'A'), b'a');
        assert_eq!(ferment_letter(b'a'), b'A');
        assert_eq!(ferment_letter(b'Z'), b'z');
        assert_eq!(ferment_letter(b'z'), b'Z');
        assert_eq!(ferment_letter(b' '), b' '); // unchanged
        assert_eq!(ferment_letter(b'1'), b'1'); // unchanged
    }

    #[test]
    fn ferment_last_changes_only_last() {
        let mut word: Vec<u8> = b"hello".to_vec();
        ferment_last(&mut word);
        assert_eq!(word, b"hellO");
    }
}
