//! Simple ZSTD dictionary trainer — top-K substrings by frequency.
//!
//! Phase 1 trainer. Scans a corpus, extracts all substrings of length
//! `MIN_SUBSTRING_LEN..=MAX_SUBSTRING_LEN`, counts frequency, and
//! concatenates the highest-scoring substrings (score = `frequency *
//! length`) until `target_size` is reached.
//!
//! This is intentionally simple — it captures most of the
//! dictionary-prefix benefit for the match finder without requiring
//! the full COVER/ZDICT algorithm. A future phase can swap in a
//! richer trainer behind the same `train_dictionary` signature.

#![forbid(unsafe_code)]

use std::collections::HashMap;

/// Minimum substring length to consider.
const MIN_SUBSTRING_LEN: usize = 8;
/// Maximum substring length to consider.
const MAX_SUBSTRING_LEN: usize = 64;
/// Cap the number of distinct substrings we track, to bound memory on
/// large corpora. Substrings beyond this count are dropped (worst
/// case: we lose some long-tail candidates).
const MAX_ENTRIES: usize = 65_536;

/// Train a dictionary blob (raw content, no header) from a corpus.
///
/// Returns at most `target_size` bytes of concatenated high-frequency
/// substrings. The caller wraps this in a [`crate::dict::ZstdDictionary`]
/// via [`crate::dict::ZstdDictionary::from_raw`].
///
/// Determinism: the same corpus + target_size always yields
/// byte-identical output. Substring selection is deterministic because
/// iteration over the `HashMap` is replaced with a sorted (frequency,
/// length, byte-order) pass before concatenation.
#[must_use]
pub fn train_dictionary(corpus: &[&[u8]], target_size: usize) -> Vec<u8> {
    if target_size == 0 || corpus.is_empty() {
        return Vec::new();
    }

    // 1. Count substring frequencies across all samples.
    let mut counts: HashMap<Vec<u8>, u32> = HashMap::new();
    for sample in corpus {
        count_substrings(sample, &mut counts);
    }

    // 2. Score = frequency * length. Sort descending by score, then by
    //    byte order for determinism (HashMap iteration order is
    //    non-deterministic in general, but the sort makes the result
    //    stable).
    let mut scored: Vec<(&[u8], u64)> = counts
        .iter()
        .map(|(sub, &freq)| (sub.as_slice(), freq as u64 * sub.len() as u64))
        .collect();

    // Sort: higher score first; tie-break by lexicographic order of
    // the substring (smaller first) for stable output.
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));

    // 3. Concatenate top substrings until we hit target_size.
    let mut out = Vec::with_capacity(target_size);
    for (sub, _) in scored {
        if out.len() + sub.len() > target_size {
            // Truncate the last substring to fit exactly.
            let room = target_size - out.len();
            out.extend_from_slice(&sub[..room]);
            break;
        }
        out.extend_from_slice(sub);
        if out.len() >= target_size {
            break;
        }
    }

    out
}

/// Count all substrings of length `MIN..=MAX` in `sample`, updating
/// `counts`. Bounded by `MAX_ENTRIES` distinct substrings.
fn count_substrings(sample: &[u8], counts: &mut HashMap<Vec<u8>, u32>) {
    if sample.len() < MIN_SUBSTRING_LEN {
        return;
    }
    // Cap positions to avoid quadratic blowup on very large samples.
    // Step by 1 for small samples, by more for large ones.
    let step = if sample.len() > 65_536 { 4 } else { 1 };
    let mut pos = 0;
    while pos + MIN_SUBSTRING_LEN <= sample.len() {
        let max_len = MAX_SUBSTRING_LEN.min(sample.len() - pos);
        // Pick a single length per position (the max for this position)
        // to bound work — captures the longest representative substring.
        let sub = &sample[pos..pos + max_len];
        if counts.len() < MAX_ENTRIES || counts.contains_key(sub) {
            *counts.entry(sub.to_vec()).or_insert(0) += 1;
        }
        pos += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::ZstdDictionary;

    #[test]
    fn empty_corpus_yields_empty_dict() {
        assert!(train_dictionary(&[], 1024).is_empty());
    }

    #[test]
    fn zero_target_yields_empty_dict() {
        let corpus: &[&[u8]] = &[b"hello world"];
        assert!(train_dictionary(corpus, 0).is_empty());
    }

    #[test]
    fn repeated_substring_appears_in_dict() {
        // Two samples sharing a common substring.
        let a = b"{\"name\":\"alice\",\"age\":30}\n";
        let b = b"{\"name\":\"bob\",\"age\":25}\n";
        let corpus: &[&[u8]] = &[a.as_slice(), b.as_slice()];
        let dict = train_dictionary(corpus, 200);
        assert!(!dict.is_empty());
        // The shared substring should be a prefix-ish region. Look for
        // a recognisable fragment.
        assert!(
            dict.windows(4).any(|w| w == b"\"nam" || w == b"name\""),
            "expected a JSON-fragment in dict, got {:?}",
            String::from_utf8_lossy(&dict)
        );
    }

    #[test]
    fn output_is_bounded_by_target_size() {
        let corpus: &[&[u8]] = &[b"abcdefghabcdefghabcdefghabcdefgh"];
        let dict = train_dictionary(corpus, 50);
        assert!(dict.len() <= 50);
    }

    #[test]
    fn determinism_same_corpus_same_output() {
        let a = b"the quick brown fox jumps over the lazy dog";
        let b = b"the quick brown dog sleeps under the lazy fox";
        let corpus: &[&[u8]] = &[a.as_slice(), b.as_slice()];
        let d1 = train_dictionary(corpus, 256);
        let d2 = train_dictionary(corpus, 256);
        assert_eq!(d1, d2, "trainer non-deterministic");
    }

    #[test]
    fn dict_from_trained_content_round_trips() {
        let corpus: &[&[u8]] = &[b"hellohello", b"worldworld"];
        let content = train_dictionary(corpus, 64);
        let dict = ZstdDictionary::from_raw(7, &content);
        let blob = dict.serialize();
        let dict2 = ZstdDictionary::deserialize(&blob).expect("deserialize");
        assert_eq!(dict, dict2);
    }
}
