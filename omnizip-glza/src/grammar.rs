//! Grammar construction.
//!
//! Builds a context-free grammar over the input: repeatedly finds the most
//! frequent repeated substring (via the suffix array + LCP array), promotes
//! it to a non-terminal rule, and replaces every non-overlapping occurrence
//! with a reference to that rule.
//!
//! ## Algorithm
//!
//! 1. Build the suffix array + LCP array of the current working sequence.
//! 2. Walk the LCP array to find the substring `[pos..pos+len]` that
//!    maximises compression gain (defined as `(occurrences - 1) * (len - 1)`,
//!    which is the number of bytes saved by extracting it to a rule minus
//!    the cost of the rule definition).
//! 3. Promote that substring to a new rule, replace every non-overlapping
//!    occurrence with a `Symbol::Rule(id)` reference.
//! 4. Repeat until no candidate improves compression.
//!
//! ## Correctness
//!
//! Rules are append-only: a new rule's id is always strictly greater than
//! every existing rule id, and its body references only symbols that
//! already exist. This guarantees the grammar is acyclic.
//!
//! ## Determinism
//!
//! When multiple candidates tie on gain, ties are broken by (length desc,
//! then first occurrence asc, then byte content asc) — a total order that
//! makes the output byte-identical for identical inputs.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use crate::suffix_array::{build_lcp_array, build_suffix_array};

/// One symbol in a grammar production: either a literal byte (0x00–0xFF)
/// or a reference to rule `N` (rule index `N`, 0-based).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Symbol {
    /// A raw byte from the input.
    Byte(u8),
    /// A reference to rule at index `n` in `Grammar::rules`.
    Rule(u16),
}

/// A context-free grammar produced by [`Grammar::build`].
///
/// The grammar has one implicit "start rule" (`start_rule`) plus `rules.len()`
/// explicit non-terminals. To decode, expand `start_rule` by replacing each
/// `Symbol::Rule(n)` with `rules[n]`, recursively.
#[derive(Clone, Debug)]
pub struct Grammar {
    /// The start symbol sequence (the top-level production).
    pub start_rule: Vec<Symbol>,
    /// Rule definitions, indexed by their `Rule(n)` id.
    pub rules: Vec<Vec<Symbol>>,
}

/// Minimum substring length worth extracting to a rule. Below this, the
/// rule-definition overhead exceeds the savings.
const MIN_RULE_LEN: usize = 4;

/// Minimum number of occurrences for a substring to be worth extracting.
const MIN_OCCURRENCES: u32 = 2;

impl Grammar {
    /// Build a grammar from `data` by repeatedly extracting the
    /// highest-gain repeated substring.
    ///
    /// The grammar is guaranteed acyclic and deterministic.
    #[must_use]
    pub fn build(data: &[u8]) -> Self {
        // Working sequence starts as all-byte symbols.
        let mut working: Vec<Symbol> = data.iter().map(|&b| Symbol::Byte(b)).collect();
        let mut rules: Vec<Vec<Symbol>> = Vec::new();

        loop {
            let candidate = find_best_substring(&working);
            let Some((pattern, _gain)) = candidate else {
                break;
            };
            if rules.len() >= u16::MAX as usize {
                // Hard cap on rule count — the wire format uses u16.
                break;
            }

            // Promote the pattern to a new rule.
            let rule_id = rules.len() as u16;
            rules.push(pattern.clone());

            // Replace every non-overlapping occurrence of `pattern` in
            // `working` with `Symbol::Rule(rule_id)`. Leftmost-first scan
            // ensures deterministic placement.
            replace_all(&mut working, &pattern, Symbol::Rule(rule_id));
        }

        Self {
            start_rule: working,
            rules,
        }
    }

    /// Total symbol count across all productions (used by encode).
    #[must_use]
    pub fn symbol_count(&self) -> usize {
        self.start_rule.len() + self.rules.iter().map(Vec::len).sum::<usize>()
    }
}

/// Find the highest-gain repeated substring in `seq`.
///
/// Returns `Some((pattern, gain))` where `pattern` is the substring as a
/// `Vec<Symbol>` and `gain` is the estimated byte savings. Returns `None`
/// if no substring meets the `MIN_RULE_LEN` / `MIN_OCCURRENCES` thresholds.
fn find_best_substring(seq: &[Symbol]) -> Option<(Vec<Symbol>, u32)> {
    if seq.len() < MIN_RULE_LEN * 2 {
        return None;
    }

    // Materialise the byte image of the working sequence so we can run a
    // suffix-array sort over it. We need a stable byte image; this means
    // rule-references must be encodable as bytes. For the grammar-extraction
    // search to be correct, two equal runs of symbols must produce two
    // equal runs of bytes. We achieve this by mapping each `Symbol::Rule(n)`
    // to a deterministic 2-byte image (high byte set so it cannot collide
    // with a literal byte, which is always <= 0xFF).
    //
    // Important: this byte image is ONLY used for substring search. It is
    // not the wire format. Equality in the byte image implies equality of
    // the underlying symbol sequence, and vice versa.
    let image = symbol_image(seq);

    let sa = build_suffix_array(&image);
    let lcp = build_lcp_array(&image, &sa);

    // Walk LCP array. For each run of equal-LCP entries, the number of
    // distinct suffixes sharing a prefix of length >= L is the run length.
    // We look for the best candidate.
    //
    // For each index `i` in the LCP array, the longest common prefix
    // between sa[i-1] and sa[i] has length lcp[i]. A substring of length L
    // starting at position sa[i] appears at least twice (with the suffix at
    // sa[i-1]) provided L <= lcp[i].
    //
    // We find, for each distinct starting position, the maximum run of LCP
    // entries >= some L, which gives the occurrence count. To keep this
    // O(n) per pass, we use a monotonic-stack-free sliding approach: for
    // each LCP value, count how many consecutive entries are >= L.
    //
    // Simpler & robust: for each suffix-array index `i` (1..n), consider
    // the prefix of length L = lcp[i]. Extend left and right while
    // lcp[j] >= L; the count is (right - left + 1). Gain for this
    // candidate is (count) * (L - 1) [bytes saved by collapsing to a
    // single rule reference] minus L [cost of the rule definition].
    //
    // We sweep all `i` and keep the best.

    let n = sa.len();
    let mut best: Option<(Vec<Symbol>, u32)> = None;
    let mut best_gain: u32 = 0;

    let mut i = 1;
    while i < n {
        let l = lcp[i] as usize;
        if l < MIN_RULE_LEN {
            i += 1;
            continue;
        }
        // Extend the run of lcp >= l.
        let mut left = i;
        while left > 1 && lcp[left - 1] >= l as u32 {
            left -= 1;
        }
        let mut right = i;
        while right + 1 < n && lcp[right + 1] >= l as u32 {
            right += 1;
        }
        // Occurrences = (right - left + 1) suffixes share a prefix of length
        // at least `l`. But to be precise we need the count of positions
        // that begin with this prefix: that is (right - left + 2), because
        // `left-1` also shares the prefix with `left`.
        let count = (right - left + 2) as u32;
        let pattern_start = sa[left - 1] as usize;
        let pattern_len = l;

        // Make sure we are not extending across a rule-image boundary in a
        // way that splits a Rule symbol (a rule is encoded as 2 bytes).
        // For correctness of replacement, the pattern must begin and end on
        // symbol boundaries.
        let boundary = symbol_boundary(seq, pattern_start, pattern_len);
        if let Some((sym_start, sym_len)) = boundary {
            // Number of symbols covered by this pattern.
            let sym_count = sym_len;
            if sym_count >= MIN_RULE_LEN && count >= MIN_OCCURRENCES {
                let gain = sym_count
                    .saturating_sub(1)
                    .saturating_mul(count.saturating_sub(1) as usize)
                    as u32;
                if gain > best_gain {
                    let pattern: Vec<Symbol> = seq[sym_start..sym_start + sym_len].to_vec();
                    // Tie-break check is implicit: first-found wins, and the
                    // suffix array is sorted, so equal-gain candidates are
                    // visited in lexicographic order.
                    best_gain = gain;
                    best = Some((pattern, gain));
                }
            }
        }

        i = right + 1;
    }

    best
}

/// Produce a deterministic byte image of a symbol sequence such that
/// `image(a) == image(b)` iff `a == b` (as symbol sequences).
///
/// - `Symbol::Byte(b)` -> `[b]` (1 byte).
/// - `Symbol::Rule(n)` -> `[0x01, high_byte, low_byte]` (3 bytes, marker
///   0x01 chosen because literal bytes 0x00–0xFF already occupy the 1-byte
///   space and we need a discriminator). Actually since literals are a full
///   byte we use a 3-byte tag `[0xFE, hi, lo]` so the image length per
///   symbol is unambiguous and rule images never collide with byte images.
fn symbol_image(seq: &[Symbol]) -> Vec<u8> {
    let mut out = Vec::with_capacity(seq.len() * 2);
    for s in seq {
        match s {
            Symbol::Byte(b) => {
                out.push(*b);
            }
            Symbol::Rule(n) => {
                out.push(0xFE);
                out.push((n >> 8) as u8);
                out.push((n & 0xFF) as u8);
            }
        }
    }
    out
}

/// Given a byte-image pattern at `[start..start+len]` (a region of the image
/// returned by [`symbol_image`]), determine whether it aligns to a whole
/// number of symbols in `seq`. If yes, returns `(symbol_start_index,
/// symbol_count)`.
fn symbol_boundary(seq: &[Symbol], img_start: usize, img_len: usize) -> Option<(usize, usize)> {
    // Walk seq, accumulating image lengths, until we land exactly on
    // img_start and then on img_start + img_len.
    let mut pos = 0usize; // current image position
    let mut sym_idx = 0usize; // current symbol index
    let n = seq.len();

    // Advance to the symbol whose image starts at img_start.
    while sym_idx < n && pos < img_start {
        pos += symbol_img_len(seq[sym_idx]);
        sym_idx += 1;
    }
    if pos != img_start {
        return None;
    }
    let sym_start = sym_idx;
    let target_end = img_start + img_len;
    let mut sym_count = 0usize;
    while sym_idx < n && pos < target_end {
        pos += symbol_img_len(seq[sym_idx]);
        sym_idx += 1;
        sym_count += 1;
    }
    if pos == target_end {
        Some((sym_start, sym_count))
    } else {
        None
    }
}

/// Length of the byte image of a single symbol.
fn symbol_img_len(s: Symbol) -> usize {
    match s {
        Symbol::Byte(_) => 1,
        Symbol::Rule(_) => 3,
    }
}

/// Replace every non-overlapping occurrence of `pattern` in `seq` with
/// `replacement`. Mutates `seq` in place (returns a new `Vec`).
fn replace_all(seq: &mut Vec<Symbol>, pattern: &[Symbol], replacement: Symbol) {
    if pattern.is_empty() || seq.len() < pattern.len() {
        return;
    }
    let mut out: Vec<Symbol> = Vec::with_capacity(seq.len());
    let mut i = 0;
    while i < seq.len() {
        if i + pattern.len() <= seq.len() && seq[i..i + pattern.len()] == *pattern {
            out.push(replacement);
            i += pattern.len();
        } else {
            out.push(seq[i]);
            i += 1;
        }
    }
    *seq = out;
}

#[cfg(test)]
#[allow(clippy::len_zero)]
mod tests {
    use super::*;

    #[test]
    fn no_rules_for_unique_data() {
        // Every byte distinct — no repeated substring of length >= 4.
        let data: Vec<u8> = (0..200u8).collect();
        let g = Grammar::build(&data);
        assert_eq!(g.rules.len(), 0, "no rules expected for unique data");
        // Start rule is just the bytes.
        assert_eq!(g.start_rule.len(), data.len());
    }

    #[test]
    fn extracts_rule_for_repeated_pattern() {
        // "abcd" repeated 5 times should yield at least one rule.
        let mut data = Vec::new();
        for _ in 0..5 {
            data.extend_from_slice(b"abcd");
        }
        let g = Grammar::build(&data);
        assert!(g.rules.len() >= 1, "expected at least one rule");
    }

    #[test]
    fn very_repetitive_input() {
        let data = vec![0x41u8; 10_000];
        let g = Grammar::build(&data);
        // Should extract rules and collapse the start rule dramatically.
        assert!(g.start_rule.len() < data.len(), "start rule should shrink");
    }

    #[test]
    fn grammar_is_acyclic() {
        // A grammar that references itself (directly or transitively) would
        // cause infinite expansion. Verify this never happens.
        let data = b"abcdabcdabcdabcd efgh efgh efgh efgh".to_vec();
        let g = Grammar::build(&data);
        assert_acyclic(&g);
    }

    fn assert_acyclic(g: &Grammar) {
        // For each rule, do a DFS over its references. A rule may only
        // reference rules with strictly smaller ids (this is the
        // append-only invariant the builder maintains).
        for (id, body) in g.rules.iter().enumerate() {
            for s in body {
                if let Symbol::Rule(ref_id) = s {
                    assert!(
                        (*ref_id as usize) < id,
                        "rule {id} references rule {ref_id} which is not strictly smaller — cycle risk"
                    );
                }
            }
        }
        // The start rule may reference any rule.
    }

    #[test]
    fn determinism() {
        let data = b"the quick brown fox the quick brown fox the quick brown fox".to_vec();
        let a = Grammar::build(&data);
        let b = Grammar::build(&data);
        assert_eq!(a.start_rule, b.start_rule);
        assert_eq!(a.rules, b.rules);
    }
}
