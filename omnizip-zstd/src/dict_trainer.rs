//! ZSTD dictionary trainers — pluggable behind the [`DictTrainer`] trait.
//!
//! Two implementations:
//!
//! - [`FrequencyTrainer`] — top-K substrings by frequency × length.
//!   Cheap, deterministic, captures obvious common substrings. This is
//!   the original omnizip-rs trainer; retained as the default.
//! - [`FastCoverTrainer`] — dmer-frequency scoring per the FastCover
//!   algorithm (Facebook 2018). Each K-byte segment is scored by the
//!   sum of D-byte dmer frequencies inside it; the top segments are
//!   concatenated to form the dictionary body. Better ratio than
//!   `FrequencyTrainer` on corpora with distributed redundancy (mixed
//!   JSON, source files, log lines).
//!
//! Adding a trainer = one more `impl DictTrainer`. No edits to the
//! dictionary-aware compress/decompress paths (open/closed).
//!
//! ## Determinism
//!
//! Both trainers are deterministic: same samples + same target_size +
//! same options ⇒ byte-identical dictionary. Hash iteration is replaced
//! with sorted passes; ties in scoring are broken by `(sample_index,
//! offset)` ordering.

#![forbid(unsafe_code)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use omnizip_codecs::hash::fnv1a_32;

/// A dictionary-training algorithm.
///
/// Implementations produce a raw dictionary body (no header) of at
/// most `target_size` bytes from a corpus of samples. The caller wraps
/// the result in a [`crate::dict::ZstdDictionary`] via
/// [`crate::dict::ZstdDictionary::from_raw`].
pub trait DictTrainer {
    /// Train a dictionary body from `samples`.
    fn train(&self, samples: &[&[u8]], target_size: usize) -> Vec<u8>;
}

/// Backward-compat free function: trains with the default
/// [`FrequencyTrainer`].
///
/// Existing callers (`pub use dict_trainer::train_dictionary`) keep
/// their signature. New code should construct a trainer explicitly
/// and call [`DictTrainer::train`] for clarity.
#[must_use]
pub fn train_dictionary(samples: &[&[u8]], target_size: usize) -> Vec<u8> {
    FrequencyTrainer::new().train(samples, target_size)
}

/// Train with any [`DictTrainer`] impl.
#[must_use]
pub fn train_dictionary_with(
    trainer: &dyn DictTrainer,
    samples: &[&[u8]],
    target_size: usize,
) -> Vec<u8> {
    trainer.train(samples, target_size)
}

// ---------------------------------------------------------------------------
// FrequencyTrainer — top-K substrings by frequency × length.
// ---------------------------------------------------------------------------

/// Minimum substring length considered by [`FrequencyTrainer`].
const FREQ_MIN_LEN: usize = 8;
/// Maximum substring length considered by [`FrequencyTrainer`].
const FREQ_MAX_LEN: usize = 64;
/// Cap distinct substrings tracked, to bound memory on large corpora.
const FREQ_MAX_ENTRIES: usize = 65_536;

/// Top-K substring trainer. Original omnizip-rs implementation.
pub struct FrequencyTrainer;

impl Default for FrequencyTrainer {
    fn default() -> Self {
        Self::new()
    }
}

impl FrequencyTrainer {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DictTrainer for FrequencyTrainer {
    fn train(&self, samples: &[&[u8]], target_size: usize) -> Vec<u8> {
        if target_size == 0 || samples.is_empty() {
            return Vec::new();
        }

        let mut counts: std::collections::HashMap<Vec<u8>, u32> = std::collections::HashMap::new();
        for sample in samples {
            count_substrings(sample, &mut counts);
        }

        let mut scored: Vec<(&[u8], u64)> = counts
            .iter()
            .map(|(sub, &freq)| (sub.as_slice(), freq as u64 * sub.len() as u64))
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));

        let mut out = Vec::with_capacity(target_size);
        for (sub, _) in scored {
            if out.len() + sub.len() > target_size {
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
}

fn count_substrings(sample: &[u8], counts: &mut std::collections::HashMap<Vec<u8>, u32>) {
    if sample.len() < FREQ_MIN_LEN {
        return;
    }
    let step = if sample.len() > 65_536 { 4 } else { 1 };
    let mut pos = 0;
    while pos + FREQ_MIN_LEN <= sample.len() {
        let max_len = FREQ_MAX_LEN.min(sample.len() - pos);
        let sub = &sample[pos..pos + max_len];
        if counts.len() < FREQ_MAX_ENTRIES || counts.contains_key(sub) {
            *counts.entry(sub.to_vec()).or_insert(0) += 1;
        }
        pos += step;
    }
}

// ---------------------------------------------------------------------------
// FastCoverTrainer — dmer-frequency segment scoring.
// ---------------------------------------------------------------------------

/// Tuning knobs for [`FastCoverTrainer`].
#[derive(Debug, Clone, Copy)]
pub struct FastCoverOptions {
    /// Segment size K (the unit of dict assembly). Default 200, matching
    /// upstream `zstd --train` defaults.
    pub k: usize,
    /// Dmer size D (substrings scored inside each segment). Default 8.
    pub d: usize,
}

impl Default for FastCoverOptions {
    fn default() -> Self {
        Self { k: 200, d: 8 }
    }
}

impl FastCoverOptions {
    /// Sensible small-corpus defaults (K=64, D=6). Better than the
    /// upstream default when the corpus is only a few KB.
    #[must_use]
    pub const fn small() -> Self {
        Self { k: 64, d: 6 }
    }
}

/// Hash-table size = `2^LOG_TABLE`. 16 ⇒ 65 536 buckets — enough for
/// most corpora without exhausting memory.
const LOG_TABLE: u32 = 16;
const TABLE_SIZE: usize = 1 << LOG_TABLE;
const TABLE_MASK: usize = TABLE_SIZE - 1;

/// FastCover trainer: scores each K-byte segment by the sum of its
/// D-byte dmer frequencies, then concatenates the highest-scoring
/// segments.
///
/// This is a faithful but simplified FastCover: it omits the inner
/// optimisation loop that tries several `(K, D)` combinations and
/// picks the best by validation-set ratio. Callers wanting that should
/// shell out to `zstd --train` and feed the result via
/// [`crate::dict::ZstdDictionary::deserialize`].
pub struct FastCoverTrainer {
    opts: FastCoverOptions,
}

impl Default for FastCoverTrainer {
    fn default() -> Self {
        Self::new(FastCoverOptions::default())
    }
}

impl FastCoverTrainer {
    #[must_use]
    pub const fn new(opts: FastCoverOptions) -> Self {
        Self { opts }
    }
}

impl DictTrainer for FastCoverTrainer {
    fn train(&self, samples: &[&[u8]], target_size: usize) -> Vec<u8> {
        if target_size == 0 || samples.is_empty() {
            return Vec::new();
        }
        // Clamp K to the longest sample so we still extract segments
        // from small corpora (otherwise the trainer silently produces
        // nothing, which is correct but useless).
        let longest = samples.iter().map(|s| s.len()).max().unwrap_or(0);
        if longest < self.opts.d {
            return Vec::new();
        }
        let k = self.opts.k.max(8).min(longest);
        let d = self.opts.d.clamp(4, 16).min(k);

        let mut counts = vec![0u32; TABLE_SIZE];
        for sample in samples {
            count_dmers(sample, d, &mut counts);
        }

        // Score each segment: sum of dmer counts within it.
        let mut segments: Vec<(usize, usize, u64)> = Vec::new();
        for (si, sample) in samples.iter().enumerate() {
            if sample.len() < k {
                continue;
            }
            let stride = k;
            let mut off = 0;
            while off + k <= sample.len() {
                let mut score: u64 = 0;
                let mut p = off;
                while p + d <= off + k {
                    let h = dmer_hash(&sample[p..p + d]);
                    score = score.saturating_add(u64::from(counts[h]));
                    p += d;
                }
                segments.push((si, off, score));
                off += stride;
            }
        }
        // Sort: highest score first; tie-break by (sample_idx, offset)
        // for full determinism.
        segments.sort_by(|a, b| b.2.cmp(&a.2).then((a.0, a.1).cmp(&(b.0, b.1))));

        let mut out = Vec::with_capacity(target_size);
        for (si, off, _) in segments {
            let sample = samples[si];
            let end = (off + k).min(sample.len());
            let seg = &sample[off..end];
            if out.len() + seg.len() > target_size {
                let room = target_size - out.len();
                out.extend_from_slice(&seg[..room]);
                break;
            }
            out.extend_from_slice(seg);
            if out.len() >= target_size {
                break;
            }
        }
        out
    }
}

fn count_dmers(sample: &[u8], d: usize, counts: &mut [u32]) {
    if sample.len() < d {
        return;
    }
    let mut p = 0;
    while p + d <= sample.len() {
        let h = dmer_hash(&sample[p..p + d]);
        counts[h] = counts[h].saturating_add(1);
        p += d;
    }
}

/// Dmer hash: FNV-1a 32-bit folded into the table index. Deterministic
/// and collision-spread (FNV's avalanche is sufficient for a 16-bit
/// table — we're not relying on cryptographic strength here).
fn dmer_hash(dmer: &[u8]) -> usize {
    let h = fnv1a_32(dmer);
    // Fold the high 16 bits into the low 16 bits to use the full table
    // even when the low bits are correlated (common for short dmers).
    let folded = (h ^ (h >> 16)) & (TABLE_MASK as u32);
    folded as usize
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::ZstdDictionary;

    // --- FrequencyTrainer (preserved behaviour) ---

    #[test]
    fn freq_empty_corpus_yields_empty_dict() {
        assert!(FrequencyTrainer::new().train(&[], 1024).is_empty());
    }

    #[test]
    fn freq_zero_target_yields_empty_dict() {
        assert!(FrequencyTrainer::new().train(&[b"hello"], 0).is_empty());
    }

    #[test]
    fn freq_repeated_substring_appears() {
        let a = b"{\"name\":\"alice\",\"age\":30}\n";
        let b = b"{\"name\":\"bob\",\"age\":25}\n";
        let dict = FrequencyTrainer::new().train(&[a.as_slice(), b.as_slice()], 200);
        assert!(!dict.is_empty());
        assert!(
            dict.windows(4).any(|w| w == b"\"nam" || w == b"name\""),
            "expected a JSON fragment, got {:?}",
            String::from_utf8_lossy(&dict)
        );
    }

    #[test]
    fn freq_determinism() {
        let a = b"the quick brown fox jumps over the lazy dog";
        let b = b"the quick brown dog sleeps under the lazy fox";
        let d1 = FrequencyTrainer::new().train(&[a.as_slice(), b.as_slice()], 256);
        let d2 = FrequencyTrainer::new().train(&[a.as_slice(), b.as_slice()], 256);
        assert_eq!(d1, d2);
    }

    // --- FastCoverTrainer ---

    #[test]
    fn fastcover_empty_corpus_yields_empty_dict() {
        assert!(FastCoverTrainer::default().train(&[], 1024).is_empty());
    }

    #[test]
    fn fastcover_zero_target_yields_empty_dict() {
        assert!(FastCoverTrainer::default()
            .train(&[b"hello world this is long enough"], 0)
            .is_empty());
    }

    #[test]
    fn fastcover_output_bounded_by_target_size() {
        let s = b"function handler() { return CONSTANT + SHARED_SUFFIX; }\n".repeat(20);
        let dict = FastCoverTrainer::default().train(&[s.as_slice()], 50);
        assert!(dict.len() <= 50);
    }

    #[test]
    fn fastcover_determinism() {
        let a = b"function a() { return SHARED; }\nfunction b() { return SHARED; }\n".repeat(5);
        let b = b"function c() { return SHARED; }\nfunction d() { return SHARED; }\n".repeat(5);
        let d1 = FastCoverTrainer::default().train(&[a.as_slice(), b.as_slice()], 512);
        let d2 = FastCoverTrainer::default().train(&[a.as_slice(), b.as_slice()], 512);
        assert_eq!(d1, d2, "FastCover non-deterministic");
    }

    #[test]
    fn fastcover_picks_redundant_segments_first() {
        // Two samples that share a long prefix; FastCover should put
        // that prefix at the start of the dict.
        let prefix = b"COMMON_PREFIX_BYTES_THAT_APPEARS_EVERYWHERE_IN_THESE_SAMPLES_";
        let alpha = b"alpha alpha alpha alpha alpha alpha alpha alpha";
        let beta = b"beta beta beta beta beta beta beta beta beta beta beta";
        let mut a: Vec<u8> = prefix.to_vec();
        a.extend_from_slice(alpha);
        let mut b: Vec<u8> = prefix.to_vec();
        b.extend_from_slice(beta);
        // Use a small-K trainer so a single segment covers the prefix.
        let trainer = FastCoverTrainer::new(FastCoverOptions {
            k: prefix.len(),
            d: 8,
        });
        let dict = trainer.train(&[a.as_slice(), b.as_slice()], prefix.len());
        assert!(
            dict.starts_with(b"COMMON"),
            "expected dict to start with the shared prefix, got {:?}",
            String::from_utf8_lossy(&dict)
        );
    }

    #[test]
    fn fastcover_smaller_k_for_small_corpus() {
        let samples: Vec<Vec<u8>> = (0..20)
            .map(|i| {
                format!("item_{i:03}: shared_token_value_and_some_padding_to_reach_k\n")
                    .into_bytes()
            })
            .collect();
        let refs: Vec<&[u8]> = samples.iter().map(Vec::as_slice).collect();
        let dict = FastCoverTrainer::new(FastCoverOptions::small()).train(&refs, 256);
        assert!(!dict.is_empty());
        // Should pick up the shared token.
        assert!(
            dict.windows(5).any(|w| w == b"share" || w == b"item_"),
            "expected dict to contain a common token, got {:?}",
            String::from_utf8_lossy(&dict)
        );
    }

    // --- Backward-compat ---

    #[test]
    fn train_dictionary_function_uses_frequency_trainer() {
        // The free function delegates to FrequencyTrainer — this guards
        // the existing API contract.
        let corpus: &[&[u8]] = &[b"abcdefghabcdefghabcdefghabcdefgh"];
        let via_function = train_dictionary(corpus, 50);
        let via_struct = FrequencyTrainer::new().train(corpus, 50);
        assert_eq!(via_function, via_struct);
    }

    #[test]
    fn trained_dict_round_trips_through_serialization() {
        let corpus: &[&[u8]] = &[b"hellohello", b"worldworld"];
        let content = train_dictionary(corpus, 64);
        let dict = ZstdDictionary::from_raw(7, &content);
        let blob = dict.serialize();
        let dict2 = ZstdDictionary::deserialize(&blob).expect("deserialize");
        assert_eq!(dict, dict2);
    }

    #[test]
    fn train_dictionary_with_trait_dispatch() {
        let corpus: &[&[u8]] = &[b"abcabcabc", b"xyzxyzxyz"];
        let freq = train_dictionary_with(&FrequencyTrainer::new(), corpus, 32);
        let fast = train_dictionary_with(&FastCoverTrainer::default(), corpus, 32);
        assert!(!freq.is_empty() || !fast.is_empty());
    }
}
