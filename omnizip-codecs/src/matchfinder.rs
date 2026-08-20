//! Reusable hash-chain LZ77 match finder.
//!
//! Shared by `omnizip-lzma`, `omnizip-lz4` (HC mode), `omnizip-libdeflate`,
//! and `omnizip-zstd`. Each codec keeps its own LZ77 token format
//! (ZSTD's `RawSequence`, LZMA's `Match`, etc.) and adapts it to this
//! generic finder — the finder itself is codec-agnostic.
//!
//! ## Algorithm
//!
//! Hash-chain with word-at-a-time match extension:
//!
//! 1. **Hash** 4 bytes at the current position into a `u32` hash.
//! 2. **Probe**: look up `head[hash]` for the most recent position with
//!    the same hash.
//! 3. **Verify** the 4-byte prefix matches.
//! 4. **Extend** forward using `u64` XOR + `trailing_zeros` (5-8×
//!    faster than byte-by-byte).
//! 5. **Walk the chain** (`prev[pos]`) up to `max_chain_length` entries
//!    to find longer matches, exiting early once a match of length
//!    ≥ `nice_match` is found.
//! 6. **Insert** the current position into the hash table + chain for
//!    future queries.
//!
//! ## Determinism
//!
//! All data structures are pre-allocated per finder invocation. No
//! `HashSet` iteration, no thread-local state, no `DefaultHasher`.

#![forbid(unsafe_code)]

/// A potential LZ77 match found by [`HashChainMatchFinder::find_match`].
///
/// Codec-agnostic. Distance is 1-based (distance=1 means the previous
/// byte), matching DEFLATE / LZ4 / ZSTD conventions. Length excludes
/// any minimum-match offset (callers add their own `MIN_MATCH`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lz77Match {
    /// 1-based distance back from the current position.
    pub distance: u32,
    /// Match length in bytes (≥ `min_match`).
    pub length: u32,
}

/// Configuration knobs for [`HashChainMatchFinder`].
///
/// Codecs construct one of these to express their strategy. Sensible
/// defaults are provided via [`HashChainConfig::default`]; per-codec
/// cparams tables override individual fields.
#[derive(Clone, Copy, Debug)]
pub struct HashChainConfig {
    /// Effective dictionary size in bytes. Bounds both the hash
    /// table size and the maximum match distance.
    pub dict_size: u32,
    /// Minimum useful match length. Matches shorter than this are
    /// discarded by [`find_match`](HashChainMatchFinder::find_match).
    pub min_match: u32,
    /// Maximum hash-chain entries to walk per position. 0 disables
    /// chain walking (single-probe, "fast" strategy).
    pub max_chain_length: u32,
    /// Stop the chain walk once a match this long is found. 0
    /// disables early exit.
    pub nice_match: u32,
    /// Hash table size = `1 << hash_log`. Larger = fewer collisions
    /// but more memory and zero-init cost.
    pub hash_log: u32,
    /// Bytes mixed into the bucket hash (default 4). A companion
    /// finder with 6 keeps long-structure chains clean when 4-byte
    /// buckets congest at scale (upstream H6's dual-bank idea).
    pub hash_bytes: u32,
    /// Upper bound on `match_length` scan per candidate. 0 disables
    /// the cap (scans up to end-of-input). Set to the codec's max
    /// useful match length (e.g., brotli's MAX_COPY=271) to avoid
    /// O(N²) blowup on highly repetitive inputs where match_length
    /// would otherwise walk O(N) bytes per call.
    pub max_match_length: u32,
}

impl Default for HashChainConfig {
    fn default() -> Self {
        Self {
            dict_size: 1 << 16,
            min_match: 3,
            max_chain_length: 128,
            nice_match: 32,
            hash_log: 16,
            hash_bytes: 4,
            max_match_length: 0,
        }
    }
}

/// Multiplicative hash prime for 4-byte LZ77 hashing. Mirrors
/// `ZSTD_prime4bytes` and `xz-utils`'s hash multiplier.
const HASH_PRIME: u32 = 0x9E37_79B1;

/// Hash-chain LZ77 match finder.
///
/// Holds references into the input data plus the hash/chain arrays.
/// Reusable across blocks within a single frame via [`reset`](Self::reset);
/// reusable across frames via [`resize_for`](Self::resize_for).
///
/// ## Performance
///
/// Word-at-a-time match extension via `u64::from_le_bytes` and
/// `trailing_zeros` for the first mismatch byte. 5-8× faster than
/// byte-by-byte on typical inputs.
pub struct HashChainMatchFinder<'a> {
    data: &'a [u8],
    head: Vec<u32>,
    prev: Vec<u32>,
    mask: u32,
    hash_log: u32,
    hash_bytes: u32,
    cur: usize,
    max_distance: u32,
    max_chain_length: u32,
    min_match: u32,
    nice_match: u32,
    max_match_length: u32,
}

const SENTINEL: u32 = u32::MAX;

impl<'a> HashChainMatchFinder<'a> {
    /// Construct a match finder over `data` with the given `config`.
    #[must_use]
    pub fn new(data: &'a [u8], config: HashChainConfig) -> Self {
        let dict_size = config.dict_size.max(4096);
        // The prev[] chain array MUST be indexed by a power-of-two mask:
        // a non-power-of-two dict_size (e.g. (1<<24)-16) yields a mask
        // with clear bits, aliasing positions 2^k apart and scrambling
        // the chains — long-distance matches silently vanish. The
        // window/dictionary VALIDITY limit (max_distance) stays at
        // dict_size; only the indexing space is rounded up.
        let prev_size = (dict_size as usize).next_power_of_two();
        let mask = (prev_size - 1) as u32;
        Self {
            data,
            head: vec![SENTINEL; 1usize << config.hash_log],
            prev: vec![SENTINEL; prev_size],
            mask,
            hash_log: config.hash_log,
            hash_bytes: config.hash_bytes,
            cur: 0,
            max_distance: dict_size,
            max_chain_length: config.max_chain_length,
            min_match: config.min_match,
            nice_match: config.nice_match,
            max_match_length: config.max_match_length,
        }
    }

    /// Current position in the input data.
    /// Configured chain-walk depth.
    #[must_use]
    pub const fn max_chain_length(&self) -> u32 {
        self.max_chain_length
    }

    #[must_use]
    pub const fn position(&self) -> usize {
        self.cur
    }

    /// Total input length.
    #[must_use]
    pub const fn input_len(&self) -> usize {
        self.data.len()
    }

    /// Advance to the next position, inserting the current position
    /// into the hash table and chain. Returns the position advanced
    /// to, or `None` at end-of-input.
    pub fn advance(&mut self) -> Option<usize> {
        if self.cur >= self.data.len() {
            return None;
        }
        self.insert_at(self.cur);
        let pos = self.cur;
        self.cur += 1;
        Some(pos)
    }

    /// Find the longest match at `pos`. Walks the chain up to
    /// `max_chain_length` entries, exiting early once a match ≥
    /// `nice_match` is found.
    ///
    /// Returns `None` if no candidate yields a match ≥ `min_match`.
    ///
    /// Handles the case where [`advance`](Self::advance) was already
    /// called for `pos` (inserting `pos` into the hash chain). In that
    /// case `head[hash(pos)] == pos`, so the first candidate is the
    /// current position itself — we follow `prev[pos]` to skip it.
    #[must_use]
    pub fn find_match(&self, pos: usize) -> Option<Lz77Match> {
        self.find_match_capped(pos, self.max_chain_length)
    }

    /// Chain walk capped below the configured depth — for approximate
    /// lookahead where the deferral decision tolerates a shorter best.
    pub fn find_match_capped(&self, pos: usize, cap: u32) -> Option<Lz77Match> {
        if pos + 4 > self.data.len() {
            return None;
        }
        let h = Self::hash_n(self.data, pos, self.hash_log, self.hash_bytes);
        let mut candidate = self.head[h];
        // If advance() already inserted pos, head[h] == pos and we'd
        // compute dist=0 (matching ourselves). Skip to the previous
        // entry in the chain.
        if candidate == pos as u32 {
            candidate = self.prev[pos & self.mask as usize];
        }
        let mut best_len = 0u32;
        let mut best_dist = 0u32;
        let mut chain = 0u32;
        let max_len = if self.max_match_length > 0 {
            ((self.data.len() - pos) as u32).min(self.max_match_length)
        } else {
            (self.data.len() - pos) as u32
        };

        while candidate != SENTINEL && chain < cap {
            let cand_us = candidate as usize;
            let dist = pos.saturating_sub(cand_us);
            if dist == 0 || dist as u32 > self.max_distance {
                break;
            }

            let len = Self::match_length(self.data, pos, cand_us, max_len);
            if len > best_len && len >= self.min_match {
                best_len = len;
                best_dist = dist as u32;
                if len >= max_len {
                    break;
                }
                if self.nice_match > 0 && len >= self.nice_match {
                    break;
                }
            }

            candidate = self.prev[cand_us & self.mask as usize];
            chain += 1;
        }

        if best_len >= self.min_match {
            Some(Lz77Match {
                distance: best_dist,
                length: best_len,
            })
        } else {
            None
        }
    }

    /// Reset the finder for re-use with new data. The hash table and
    /// chain are zeroed (sentinel-filled) so no stale matches leak.
    pub fn reset(&mut self) {
        for h in &mut self.head {
            *h = SENTINEL;
        }
        for p in &mut self.prev {
            *p = SENTINEL;
        }
        self.cur = 0;
    }

    /// Override the chain-walk depth. 0 = single-probe (fast strategy).
    pub fn set_max_chain_length(&mut self, n: u32) {
        self.max_chain_length = n;
    }

    /// Override the early-exit match length. 0 = disabled.
    pub fn set_nice_match(&mut self, n: u32) {
        self.nice_match = n;
    }

    /// Find the CLOSEST match (smallest distance) at `pos` with length >=
    /// `min_len`. Walks the chain in order (most-recent first) and returns
    /// the first candidate that yields a match of at least `min_len` bytes.
    /// Useful for rep-code-aware parsing where closest matches are preferred
    /// because they're more likely to become rep0 in subsequent positions.
    #[must_use]
    pub fn find_closest_match(&self, pos: usize, min_len: u32) -> Option<Lz77Match> {
        if pos + 4 > self.data.len() {
            return None;
        }
        let h = Self::hash_n(self.data, pos, self.hash_log, self.hash_bytes);
        let mut candidate = self.head[h];
        if candidate == pos as u32 {
            candidate = self.prev[pos & self.mask as usize];
        }
        // Walk at most 8 chain entries to find a close match ≥ min_len.
        let max_walk = 8u32;
        let mut chain = 0u32;
        let max_len = if self.max_match_length > 0 {
            ((self.data.len() - pos) as u32).min(self.max_match_length)
        } else {
            (self.data.len() - pos) as u32
        };
        let limit = min_len.min(max_len);
        while candidate != SENTINEL && chain < max_walk {
            let cand_us = candidate as usize;
            let dist = pos.saturating_sub(cand_us);
            if dist == 0 || dist as u32 > self.max_distance {
                break;
            }
            let len = Self::match_length(self.data, pos, cand_us, max_len);
            if len >= limit {
                return Some(Lz77Match {
                    distance: dist as u32,
                    length: len,
                });
            }
            candidate = self.prev[cand_us & self.mask as usize];
            chain += 1;
        }
        None
    }

    /// Measure the match length between two absolute positions in the
    /// underlying data, capped at `max_len`. Used by rep-code-aware
    /// parsers to evaluate whether a stored rep distance yields a
    /// usable match at `pos`.
    #[must_use]
    pub fn match_len_between(&self, pos: usize, back: usize, max_len: u32) -> u32 {
        if back >= pos || pos >= self.data.len() {
            return 0;
        }
        Self::match_length(self.data, pos, back, max_len)
    }

    /// Collect up to `max_count` candidate matches at `pos`, one per
    /// DISTINCT distance, walking the hash chain newest-first (up to
    /// `max_walk` entries). Unlike [`find_match`](Self::find_match),
    /// which returns only the longest match, this exposes distance
    /// diversity — letting a cost-aware parser evaluate (and warm as
    /// rep codes) distances that share the top match length. On data
    /// with several repeating structures at different periods, the
    /// longest-match-only policy locks the parser onto whichever chain
    /// the hash order happens to favor, even when another chain of the
    /// same length is far more stable to revisit.
    #[must_use]
    pub fn find_candidates(&self, pos: usize, max_count: usize, max_walk: u32) -> Vec<Lz77Match> {
        let mut out = Vec::with_capacity(max_count);
        self.find_candidates_into(pos, max_count, max_walk, &mut out);
        out
    }

    /// Buffer-reusing variant of [`find_candidates`](Self::find_candidates).
    /// Clears `out` and fills it with the up-to-`max_count` LONGEST
    /// candidate matches (sorted by length descending), walking at most
    /// `max_walk` chain entries.
    ///
    /// Selection is top-K by LENGTH, not first-K by recency: on data
    /// with a periodic structure, the chain for a given 4-gram is
    /// dominated by frequent short matches (e.g. a number suffix
    /// appearing in a cyclical column), while the long structural
    /// match (the full-row repeat at the structure period) sits dozens
    /// of entries deeper. First-K collection never reaches it.
    ///
    /// Early exits: (a) a match of `nice` or more bytes is found, (b)
    /// `patience` consecutive chain entries fail to improve the
    /// current K-th best — bounding the cost of walking dense chains.
    pub fn find_candidates_into(
        &self,
        pos: usize,
        max_count: usize,
        max_walk: u32,
        out: &mut Vec<Lz77Match>,
    ) {
        self.find_candidates_into_patience(pos, max_count, max_walk, 32, out)
    }

    /// `max_patience` caps the trailing chain nodes evaluated after the
    /// candidate set fills. Those nodes' fast-reject compares are pure
    /// cache misses on the prev[] walk; measured byte-identical on CSV
    /// and FITS at q5 with the cap at 8, but q10+ parses DO use them
    /// (100KB q11 regresses +6.55% at 8) — deep tiers pass 32.
    #[must_use]
    pub fn max_match_length(&self) -> u32 {
        self.max_match_length
    }

    /// Cap compare length (upstream caps at nice_match; long compares
    /// stream match-length bytes per chain node).
    pub fn set_max_match_length(&mut self, cap: u32) {
        self.max_match_length = cap;
    }

    pub fn find_candidates_into_patience(
        &self,
        pos: usize,
        max_count: usize,
        max_walk: u32,
        max_patience: u32,
        out: &mut Vec<Lz77Match>,
    ) {
        out.clear();
        if pos + 4 > self.data.len() || max_count == 0 {
            return;
        }
        let h = Self::hash_n(self.data, pos, self.hash_log, self.hash_bytes);
        let mut candidate = self.head[h];
        if candidate == pos as u32 {
            candidate = self.prev[pos & self.mask as usize];
        }
        let max_len = if self.max_match_length > 0 {
            ((self.data.len() - pos) as u32).min(self.max_match_length)
        } else {
            (self.data.len() - pos) as u32
        };
        let nice = self.nice_match.min(max_len);
        let mut patience = max_patience;
        let mut chain = 0u32;
        while candidate != SENTINEL && chain < max_walk {
            let cand_us = candidate as usize;
            let dist = pos.saturating_sub(cand_us);
            if dist == 0 || u32::try_from(dist).unwrap_or(u32::MAX) > self.max_distance {
                break;
            }
            let len = Self::match_length(self.data, pos, cand_us, max_len);
            if len >= self.min_match {
                let m = Lz77Match {
                    distance: dist as u32,
                    length: len,
                };
                // Insert sorted by length descending; keep top max_count.
                let idx = out.partition_point(|e| e.length >= len);
                if idx < max_count {
                    if out.len() < max_count {
                        out.insert(idx, m);
                    } else {
                        let kth = out.len() - 1;
                        if idx <= kth {
                            out.insert(idx, m);
                            out.truncate(max_count);
                        }
                    }
                    if len >= nice {
                        break;
                    }
                    patience = max_patience;
                } else if out.len() >= max_count {
                    // Full and not better than the K-th best.
                    if patience == 0 {
                        break;
                    }
                    patience -= 1;
                }
            }
            candidate = self.prev[cand_us & self.mask as usize];
            chain += 1;
        }
    }

    /// Re-bind to new data, reusing the existing hash/chain allocations
    /// if the `dict_size` is unchanged. Grows them if the new dict is
    /// larger. Equivalent to `drop` + `new` but avoids reallocation.
    pub fn reuse(&mut self, data: &'a [u8], dict_size: u32) {
        let dict_size = dict_size.max(4096);
        // Power-of-two indexing space (see `new`): a non-power-of-two
        // size would alias chain entries.
        let prev_size = (dict_size as usize).next_power_of_two();
        if prev_size > self.prev.len() {
            self.prev.resize(prev_size, SENTINEL);
        }
        self.mask = (prev_size - 1) as u32;
        self.data = data;
        self.max_distance = dict_size;
        self.cur = 0;
        self.reset();
    }

    /// Hash 4 bytes at `data[pos..pos+4]` into `hash_log` bits.
    fn hash(data: &[u8], pos: usize, hash_log: u32) -> usize {
        Self::hash_n(data, pos, hash_log, 4)
    }

    /// Hash `hash_bytes` at `data[pos..]` into `hash_log` bits.
    fn hash_n(data: &[u8], pos: usize, hash_log: u32, hash_bytes: u32) -> usize {
        if pos + hash_bytes as usize > data.len() {
            return 0;
        }
        let mut acc = 0u32;
        for k in 0..hash_bytes {
            acc |= u32::from(data[pos + k as usize]) << (8 * k);
        }
        let h = acc.wrapping_mul(HASH_PRIME) >> (32 - hash_log);
        h as usize & ((1usize << hash_log) - 1)
    }

    /// Insert `pos` into the hash table + chain.
    fn insert_at(&mut self, pos: usize) {
        if pos + self.hash_bytes as usize > self.data.len() {
            return;
        }
        let h = Self::hash_n(self.data, pos, self.hash_log, self.hash_bytes);
        let prev_pos = self.head[h];
        self.prev[pos & self.mask as usize] = prev_pos;
        self.head[h] = pos as u32;
    }

    /// Word-at-a-time match length between `data[a..]` and `data[b..]`,
    /// capped at `max_len`.
    ///
    /// Hybrid strategy (TODO 277): u128 fast-reject for the first 16 bytes
    /// (eliminates short mismatches in 1 comparison), then u64 chunks for
    /// the remainder. Strictly faster than pure u64 on 64-bit targets.
    fn match_length(data: &[u8], a: usize, b: usize, max_len: u32) -> u32 {
        let max = max_len as usize;
        let mut len = 0usize;

        // u128 fast-reject: check first 16 bytes in one comparison.
        // For the majority of candidates (short or non-matches), this
        // returns immediately without entering the loop.
        if max >= 16 && a + 16 <= data.len() && b + 16 <= data.len() {
            // Single 16-byte loads: per-byte indexing costs a bounds
            // check per element and defeats load fusion.
            let wa = u128::from_le_bytes(data[a..a + 16].try_into().unwrap());
            let wb = u128::from_le_bytes(data[b..b + 16].try_into().unwrap());
            if wa != wb {
                let diff = wa ^ wb;
                return diff.trailing_zeros() as u32 / 8;
            }
            len = 16;
        }

        // u64 chunks for the remainder (fast on all 64-bit targets).
        while len + 8 <= max && a + len + 8 <= data.len() && b + len + 8 <= data.len() {
            let wa = u64::from_le_bytes(data[a + len..a + len + 8].try_into().unwrap());
            let wb = u64::from_le_bytes(data[b + len..b + len + 8].try_into().unwrap());
            if wa == wb {
                len += 8;
            } else {
                let diff = wa ^ wb;
                len += diff.trailing_zeros() as usize / 8;
                return len as u32;
            }
        }
        // Scalar tail.
        while len < max
            && a + len < data.len()
            && b + len < data.len()
            && data[a + len] == data[b + len]
        {
            len += 1;
        }
        len as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_short_match() {
        let data = b"hello world hello there";
        let cfg = HashChainConfig {
            min_match: 3,
            max_chain_length: 16,
            ..HashChainConfig::default()
        };
        let mut mf = HashChainMatchFinder::new(data, cfg);
        for _ in 0..6 {
            mf.advance();
        }
        if let Some(m) = mf.find_match(12) {
            assert_eq!(m.distance, 12);
            assert!(m.length >= 5);
        }
    }

    #[test]
    fn returns_none_at_eof() {
        let data = b"short";
        let cfg = HashChainConfig::default();
        let mut mf = HashChainMatchFinder::new(data, cfg);
        for _ in 0..data.len() {
            mf.advance();
        }
        assert!(mf.advance().is_none());
    }

    #[test]
    fn determinism_same_input_same_matches() {
        let data: Vec<u8> = (0..1000).map(|i| (i * 7 + 13) as u8).collect();
        let cfg = HashChainConfig::default();
        let find_all = || {
            let mut mf = HashChainMatchFinder::new(&data, cfg);
            let mut out = Vec::new();
            while let Some(p) = mf.advance() {
                if let Some(m) = mf.find_match(p) {
                    out.push((p, m.distance, m.length));
                }
            }
            out
        };
        assert_eq!(find_all(), find_all(), "non-deterministic");
    }

    #[test]
    fn nice_match_short_circuits_chain_walk() {
        let data: Vec<u8> = (0..8192usize).map(|i| b'a' + ((i % 4) as u8)).collect();
        let cfg = HashChainConfig {
            nice_match: 16,
            ..HashChainConfig::default()
        };
        let mut mf = HashChainMatchFinder::new(&data, cfg);
        for _ in 0..100 {
            mf.advance();
        }
        let p = mf.position();
        if let Some(m) = mf.find_match(p) {
            assert!(m.length >= 16 || m.length == (data.len() - p) as u32);
        }
    }

    #[test]
    fn match_length_word_stepping_matches_byte_stepping() {
        let data: Vec<u8> = (0..4096).map(|i| ((i * 31) % 251) as u8).collect();
        let cfg = HashChainConfig::default();
        let mut mf = HashChainMatchFinder::new(&data, cfg);
        while let Some(p) = mf.advance() {
            if let Some(m) = mf.find_match(p) {
                // Re-verify with naive byte comparison.
                let back = p - m.distance as usize;
                let mut naive = 0;
                while p + naive < data.len()
                    && back + naive < data.len()
                    && data[p + naive] == data[back + naive]
                {
                    naive += 1;
                }
                assert_eq!(naive as u32, m.length, "mismatch at pos {p}");
                if naive > 100 {
                    break;
                }
            }
        }
    }

    #[test]
    fn config_default_is_sensible() {
        let cfg = HashChainConfig::default();
        assert!(cfg.dict_size >= 4096);
        assert!(cfg.min_match >= 3);
        assert!(cfg.max_chain_length > 0);
        assert!(cfg.hash_log >= 8);
    }

    #[test]
    fn finds_match_after_advance_to_same_pos() {
        // Regression: if advance(pos) inserts pos into the hash chain,
        // find_match(pos) must skip the self-match and walk prev[].
        let data = b"hello world hello there";
        let cfg = HashChainConfig::default();
        let mut mf = HashChainMatchFinder::new(data, cfg);
        // Advance through positions 0..=12. Position 12 = "hello" which
        // matches position 0.
        for _ in 0..13 {
            mf.advance();
        }
        // find_match(12) after advance(12) — head[h] == 12, must follow prev.
        let m = mf.find_match(12).expect("should find match at pos 12");
        assert_eq!(m.distance, 12);
        assert!(m.length >= 5);
    }
}

/// Binary-tree match finder — port of the reference encoder's H10
/// `StoreAndFindMatchesH10` (c/enc/backward_references/hash_to_binary_tree).
///
/// Positions sharing a 4-byte hash bucket form a binary search tree
/// ordered by byte comparison: the walk descends left/right by the
/// first differing byte, yielding matches of strictly increasing length
/// (each at a distinct distance) and re-rooting the tree at the current
/// position as it goes. Unlike a hash chain, the tree surfaces the
/// best candidate at EVERY length tier, not just whichever entries the
/// chain happens to visit first.
pub struct BinaryTreeMatchFinder<'a> {
    data: &'a [u8],
    buckets: Vec<u32>,
    /// forest[2*pos] = left child, forest[2*pos+1] = right child.
    forest: Vec<u32>,
    invalid: u32,
}

const TREE_BUCKET_BITS: usize = 17;
const TREE_HASH_MUL32: u32 = 0x1E35_A7BD;
const TREE_MAX_COMP_LEN: usize = 128;
const TREE_DEPTH: usize = 64;

impl<'a> BinaryTreeMatchFinder<'a> {
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        let invalid = u32::MAX;
        Self {
            data,
            buckets: vec![invalid; 1 << TREE_BUCKET_BITS],
            forest: vec![invalid; data.len().saturating_mul(2)],
            invalid,
        }
    }

    fn hash_at(&self, pos: usize) -> usize {
        let b = &self.data[pos..pos + 4];
        let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        ((v.wrapping_mul(TREE_HASH_MUL32)) >> (32 - TREE_BUCKET_BITS)) as usize
    }

    fn match_len_from(&self, a: usize, b: usize, max: usize) -> usize {
        let mut l = 0usize;
        while l < max {
            match (self.data.get(a + l).copied(), self.data.get(b + l).copied()) {
                (Some(x), Some(y)) if x == y => l += 1,
                _ => break,
            }
        }
        l
    }

    /// Store `pos` into its tree, collecting matches of strictly
    /// increasing length into `out` (cleared first).
    pub fn store_and_find(&mut self, pos: usize, out: &mut Vec<Lz77Match>) {
        out.clear();
        let n = self.data.len();
        if pos + 4 > n {
            return;
        }
        let key = self.hash_at(pos);
        let mut prev_ix = self.buckets[key] as usize;
        self.buckets[key] = pos as u32;
        let mut node_left = pos * 2;
        let mut node_right = pos * 2 + 1;
        let mut best_len_left = 0usize;
        let mut best_len_right = 0usize;
        let mut depth = TREE_DEPTH;
        loop {
            let backward = pos.wrapping_sub(prev_ix);
            if prev_ix == self.invalid as usize || backward == 0 || backward > n || depth == 0 {
                self.forest[node_left] = self.invalid;
                self.forest[node_right] = self.invalid;
                break;
            }
            let cur_len = best_len_left.min(best_len_right);
            let len =
                cur_len + self.match_len_from(pos + cur_len, prev_ix + cur_len, n - pos - cur_len);
            let best_so_far = out.last().map_or(0, |m| m.length as usize);
            if len > best_so_far && len >= 4 {
                out.push(Lz77Match {
                    distance: backward as u32,
                    length: len as u32,
                });
            }
            if len >= TREE_MAX_COMP_LEN || pos + len >= n || prev_ix + len >= n {
                self.forest[node_left] = self.forest[prev_ix * 2];
                self.forest[node_right] = self.forest[prev_ix * 2 + 1];
                break;
            }
            if self.data[pos + len] > self.data[prev_ix + len] {
                best_len_left = len;
                self.forest[node_left] = prev_ix as u32;
                node_left = prev_ix * 2 + 1;
                prev_ix = self.forest[node_left] as usize;
            } else {
                best_len_right = len;
                self.forest[node_right] = prev_ix as u32;
                node_right = prev_ix * 2;
                prev_ix = self.forest[node_right] as usize;
            }
            depth -= 1;
        }
    }

    /// Longest match at `pos`.
    #[must_use]
    pub fn find_match(&mut self, pos: usize) -> Option<Lz77Match> {
        let mut out = Vec::new();
        self.store_and_find(pos, &mut out);
        out.pop()
    }

    /// Matches at `pos`, keeping the LONGEST `max_count` (the walk
    /// yields ascending lengths; a plain truncate would drop the best).
    pub fn find_candidates_into(&mut self, pos: usize, max_count: usize, out: &mut Vec<Lz77Match>) {
        self.store_and_find(pos, out);
        if out.len() > max_count {
            out.drain(..out.len() - max_count);
        }
    }

    /// Plain byte-compare between two stored positions.
    #[must_use]
    pub fn match_len_between(&self, a: usize, b: usize, max_len: u32) -> u32 {
        if b >= a || a >= self.data.len() {
            return 0;
        }
        self.match_len_from(a, b, max_len as usize) as u32
    }
}

// ---------------------------------------------------------------------------
// H5 bank hasher (port of upstream's AdvHasher<H5Sub>): a fixed-size
// circular buffer of recent positions per hash bucket. The bounded scan
// stays inside one cache line per probe — no prev[] pointer chasing.
// Scoring is upstream's exact BackwardReferenceScore family.
// ---------------------------------------------------------------------------

/// Upstream `Log2FloorNonZero`.
const fn log2_floor(v: u64) -> u32 {
    63 - v.leading_zeros()
}

pub struct BankMatchFinder<'a> {
    data: &'a [u8],
    /// Per-bucket insertion counter (upstream `num`).
    num: Vec<u16>,
    /// `1 << block_bits` most-recent positions per bucket (upstream
    /// `buckets`).
    buckets: Vec<u32>,
    bucket_bits: u32,
    block_bits: u32,
    max_distance: u32,
    /// Insert cursor (absolute position).
    cur: usize,
    /// Upstream num_last_distances_to_check: rep distances probed
    /// (with per-index penalty) before the bucket.
    num_last_dists: usize,
}

impl<'a> BankMatchFinder<'a> {
    #[must_use]
    pub fn new(data: &'a [u8], bucket_bits: u32, block_bits: u32, num_last_dists: usize) -> Self {
        let bucket_count = 1usize << bucket_bits;
        Self {
            data,
            num: vec![0; bucket_count],
            buckets: vec![u32::MAX; bucket_count << block_bits],
            bucket_bits,
            block_bits,
            max_distance: u32::MAX,
            cur: 0,
            num_last_dists,
        }
    }

    #[must_use]
    pub fn position(&self) -> usize {
        self.cur
    }

    pub fn set_max_distance(&mut self, d: u32) {
        self.max_distance = d;
    }

    #[inline]
    fn key(&self, pos: usize) -> usize {
        if pos + 4 > self.data.len() {
            return 0;
        }
        let word = u32::from_le_bytes([
            self.data[pos],
            self.data[pos + 1],
            self.data[pos + 2],
            self.data[pos + 3],
        ]);
        (word.wrapping_mul(0x1E35_A7BD) >> (32 - self.bucket_bits)) as usize // kHashMul32
    }

    fn insert(&mut self, pos: usize) {
        if pos + 4 > self.data.len() {
            return;
        }
        let key = self.key(pos);
        let mask = (1usize << self.block_bits) - 1;
        let slot = (self.num[key] as usize & mask) << self.block_bits;
        let _ = slot;
        let base = key << self.block_bits;
        self.buckets[base | (self.num[key] as usize & mask)] = pos as u32;
        self.num[key] = self.num[key].wrapping_add(1);
    }

    /// Store the cursor's position and advance (our greedy loop calls
    /// this once per position, including inside copies).
    pub fn advance(&mut self) {
        if self.cur >= self.data.len() {
            return;
        }
        self.insert(self.cur);
        self.cur += 1;
    }

    fn match_len(&self, a: usize, b: usize, limit: u32) -> u32 {
        HashChainMatchFinder::match_length(self.data, a, b, limit)
    }

    /// Upstream FindLongestMatch: rep-distance probes (scored with the
    /// last-distance bonus and per-index penalty), then the bucket scan
    /// newest-first with BackwardReferenceScore. The cursor position
    /// must already be inserted (via [`advance`](Self::advance)).
    #[must_use]
    pub fn find(&self, pos: usize, last_dists: &[u32], max_len: u32) -> Option<(u32, u32, u64)> {
        if pos + 4 > self.data.len() || max_len < 4 {
            return None;
        }
        let mut best_score = 0u64;
        let mut best: Option<(u32, u32)> = None;

        // Rep probes first (upstream BackwardReferenceScoreUsingLastDistance
        // = 135*len + 15375; penalty 39 + table per index).
        for (i, &d) in last_dists.iter().take(self.num_last_dists).enumerate() {
            if d == 0 || d as usize > pos {
                continue;
            }
            let prev = pos - d as usize;
            // Quick reject at the best length so far.
            let cur_best = best.map_or(4, |(_, l)| l) as usize;
            if pos + cur_best < self.data.len()
                && prev + cur_best < self.data.len()
                && self.data[pos + cur_best] != self.data[prev + cur_best]
            {
                continue;
            }
            let len = self.match_len(pos, prev, max_len);
            if len < 3 {
                continue;
            }
            let mut score = 135u64 * u64::from(len) + 15375;
            if i != 0 {
                score -= 39 + (0x1ca10u64 >> (i & 14)) & 14;
            }
            if score > best_score {
                best_score = score;
                best = Some((d, len));
            }
        }

        // Bucket scan, newest-first.
        let key = self.key(pos);
        let mask = (1usize << self.block_bits) - 1;
        let count = self.num[key] as usize;
        let base = key << self.block_bits;
        let bucket = &self.buckets[base..base + mask + 1];
        let down = count.saturating_sub(mask + 1);
        let mut i = count;
        while i > down {
            i -= 1;
            let prev = bucket[i & mask] as usize;
            let backward = pos.wrapping_sub(prev);
            // Uninitialized slots are u32::MAX (backward huge); skip
            // them. Only break when a REAL position is beyond the
            // window — older ring entries are then also beyond.
            if prev == usize::MAX || backward == 0 {
                continue;
            }
            if backward as u32 > self.max_distance {
                break;
            }
            let cur_best = best.map_or(3, |(_, l)| l) as usize;
            if pos + cur_best < self.data.len()
                && prev + cur_best < self.data.len()
                && self.data[pos + cur_best] != self.data[prev + cur_best]
            {
                continue;
            }
            // Upstream requires len >= 4 from the bucket scan
            // (FindMatchLengthWithLimitMin4).
            let len = self.match_len(pos, prev, max_len);
            if len >= 4 {
                let score = 135u64 * u64::from(len)
                    + 15360u64.wrapping_sub(30u64 * u64::from(log2_floor(backward as u64)));
                if score > best_score {
                    best_score = score;
                    best = Some((backward as u32, len));
                }
            }
        }
        best.map(|(d, l)| (d, l, best_score))
    }
}

#[cfg(test)]
mod bank_tests {
    use super::*;
    #[test]
    fn bank_finds_repeated_pattern() {
        // 64-byte period repeating for 4KB.
        let mut data = Vec::new();
        let pat: Vec<u8> = (0..64u8).collect();
        while data.len() < 4096 {
            data.extend_from_slice(&pat);
        }
        let mut bank = BankMatchFinder::new(&data, 14, 4, 4);
        bank.set_max_distance(u32::MAX);
        let mut last = [0u32; 4];
        let mut n_matches = 0u32;
        let mut n_period = 0u32;
        for pos in 0..data.len().saturating_sub(4) {
            bank.advance(); // inserts pos
            if let Some((d, l, _)) = bank.find(pos, &last, 128) {
                n_matches += 1;
                if d == 64 {
                    n_period += 1;
                }
                // update last like the greedy ring
                if !last.contains(&d) {
                    last.rotate_right(1);
                    last[0] = d;
                } else {
                    let k = last.iter().position(|&x| x == d).unwrap();
                    last[..=k].rotate_right(1);
                }
                let _ = l;
            }
        }
        eprintln!("matches={n_matches} period64={n_period}");
        assert!(n_matches > 1000, "too few matches: {n_matches}");
        assert!(n_period > 500, "period not found often: {n_period}");
    }
}
