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
    /// Upstream rep-scoring split: H9 (num_last_distances_to_check=16)
    /// scores short codes via kDistanceShortCodeCost; H5/H6 (4/10)
    /// use BackwardReferenceScoreUsingLastDistance (135·len + 1935 −
    /// per-index penalty) — numerically near-identical systems that
    /// differ by a few points at i ≥ 2.
    h9_scoring: bool,
    /// Upstream H6 mode (1.2.0 ChooseHasher: quality 5-9 with
    /// size_hint >= 1 MiB and lgwin >= 19): hashes 5 bytes with the
    /// 64-bit kHashMul64. H5/H58 hash 4 bytes with kHashMul32.
    hash5: bool,
}

/// Upstream kMinScore = 30·8·8 + 100 (the baseline `out.score` that
/// every candidate must strictly beat to count as a found match).
const K_MIN_SCORE: u64 = 2020;

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
            h9_scoring: num_last_dists >= 16,
            hash5: false,
        }
    }

    /// Switch to the H6 5-byte 64-bit hash (kHashMul64 << 24).
    pub fn enable_hash5(&mut self) {
        self.hash5 = true;
    }

    /// Positions a store/search needs within bounds: 8 for the H6
    /// 5-byte hash (upstream HashTypeLength), 4 otherwise.
    fn lookahead(&self) -> usize {
        if self.hash5 {
            8
        } else {
            4
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
        let look = self.lookahead();
        if pos + look > self.data.len() {
            return 0;
        }
        if self.hash5 {
            // Upstream H6 HashBytes: LOAD64LE * (kHashMul64 << 24),
            // top bucket_bits bits. Bytes 5-7 contribute 0 mod 2^64
            // (their product term shifts out), so only 5 bytes matter.
            // Built from two u32 halves: an 8-byte slice copy_from_slice
            // lowered to a memcpy call here (this runs once per stored
            // position AND once per search).
            let d = &self.data[pos..pos + 8];
            let lo = u32::from_le_bytes([d[0], d[1], d[2], d[3]]);
            let hi = u32::from_le_bytes([d[4], d[5], d[6], d[7]]);
            let v = (u64::from(hi) << 32) | u64::from(lo);
            let v = v.wrapping_mul(0x1FE3_5A7B_D357_9BD3u64 << 24);
            (v >> (64 - self.bucket_bits)) as usize
        } else {
            let word = u32::from_le_bytes([
                self.data[pos],
                self.data[pos + 1],
                self.data[pos + 2],
                self.data[pos + 3],
            ]);
            (word.wrapping_mul(0x1E35_A7BD) >> (32 - self.bucket_bits)) as usize
            // kHashMul32
        }
    }

    fn insert(&mut self, pos: usize) {
        if pos + self.lookahead() > self.data.len() {
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

    /// Store the cursor position and step — used to backfill positions
    /// skipped over by a copy.
    pub fn advance(&mut self) {
        if self.cur < self.data.len() {
            self.insert(self.cur);
            self.cur += 1;
        }
    }

    /// Step the cursor WITHOUT storing (upstream StoreRange's RLE
    /// guard skips storing the early part of overlapping copies).
    pub fn skip(&mut self) {
        if self.cur < self.data.len() {
            self.cur += 1;
        }
    }

    /// Find-AND-insert: the reference's FindLongestMatch stores the
    /// searched position into its bucket using the ONE hash it already
    /// computed for the scan. Our old split (advance + find) computed
    /// the hash twice per position.
    ///
    /// Inserts `pos`, advances the cursor past it, then scans.
    /// The lazy re-search (`insert: false`) probes WITHOUT inserting —
    /// the position gets stored when the main loop reaches it.
    #[must_use]
    pub fn find_insert(
        &mut self,
        pos: usize,
        last_dists: &[u32],
        max_len: u32,
        min_len_hint: u32,
        insert: bool,
    ) -> Option<(u32, u32, u64)> {
        if pos + self.lookahead() > self.data.len() || max_len < 2 {
            return None;
        }
        let key = self.key(pos);
        if insert && pos < self.data.len() {
            let mask = (1usize << self.block_bits) - 1;
            let base = key << self.block_bits;
            self.buckets[base | (self.num[key] as usize & mask)] = pos as u32;
            self.num[key] = self.num[key].wrapping_add(1);
            if self.cur <= pos {
                self.cur = pos + 1;
            }
        }
        self.scan_with_key(key, pos, last_dists, max_len, min_len_hint)
    }

    fn match_len(&self, a: usize, b: usize, limit: u32) -> u32 {
        HashChainMatchFinder::match_length(self.data, a, b, limit)
    }

    /// Upstream FindLongestMatch: the 16 short-code distance probes
    /// (exact reps, then rep0/rep1 ±1-3 — kDistanceCacheIndex/Offset
    /// with kDistanceShortCodeCost scoring), then the bucket scan
    /// newest-first with BackwardReferenceScore. The cursor position
    /// must already be inserted (via [`advance`](Self::advance)).
    #[must_use]
    pub fn find(&self, pos: usize, last_dists: &[u32], max_len: u32) -> Option<(u32, u32, u64)> {
        self.find_with_floor(pos, last_dists, max_len, 3)
    }

    /// Like [`find`](Self::find) with a starting reject length — the
    /// reference pre-seeds the lazy re-search at sr.len-1 so most
    /// candidates reject on one byte compare.
    pub fn find_with_floor(
        &self,
        pos: usize,
        last_dists: &[u32],
        max_len: u32,
        min_len_hint: u32,
    ) -> Option<(u32, u32, u64)> {
        if pos + self.lookahead() > self.data.len() || max_len < 2 {
            return None;
        }
        let key = self.key(pos);
        self.scan_with_key(key, pos, last_dists, max_len, min_len_hint)
    }

    fn scan_with_key(
        &self,
        key: usize,
        pos: usize,
        last_dists: &[u32],
        max_len: u32,
        min_len_hint: u32,
    ) -> Option<(u32, u32, u64)> {
        // (distance-cache index, offset) per short code — upstream
        // kDistanceCacheIndex/kDistanceCacheOffset.
        const CACHE_INDEX: [u8; 16] = [0, 1, 2, 3, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1];
        const CACHE_OFFSET: [i8; 16] = [0, 0, 0, 0, -1, 1, -2, 2, -3, 3, -1, 1, -2, 2, -3, 3];
        // Short-code cost deltas from SCORE_BASE (upstream
        // kDistanceShortCodeCost); the H9 score is
        // (SCORE_BASE + delta + 540·len) >> 2.
        const SHORT_CODE_DELTA: [i32; 16] = [
            60, -95, -117, -127, -93, -93, -96, -96, -99, -99, -105, -105, -115, -115, -125, -125,
        ];
        // Upstream seeds the search result with kMinScore: a candidate
        // only "found" if it strictly beats it.
        let mut best_score = K_MIN_SCORE;
        let mut best: Option<(u32, u32)> = None;

        // Exact-rep-only mode for shallow configs (upstream H5 probes
        // only distance_cache[0]; we keep all 4 exact reps — they are
        // worth 5.8pt on periodic data — but skip the ±1-3 offset
        // variants, which add a command per drifting chain position).
        let exact_only = self.num_last_dists <= 4;
        for i in 0..self.num_last_dists.min(16) {
            if exact_only && CACHE_OFFSET[i] != 0 {
                continue;
            }
            let base = *last_dists.get(usize::from(CACHE_INDEX[i])).unwrap_or(&0);
            let v = i64::from(base) + i64::from(CACHE_OFFSET[i]);
            if v < 1 || v as usize > pos {
                continue;
            }
            let d = v as u32;
            let prev = pos - v as usize;
            let cur_best = best.map_or(2, |(_, l)| l) as usize;
            if pos + cur_best < self.data.len()
                && prev + cur_best < self.data.len()
                && self.data[pos + cur_best] != self.data[prev + cur_best]
            {
                continue;
            }
            let len = self.match_len(pos, prev, max_len);
            // Upstream: >= 3 always, len 2 only for the two exact-rep
            // codes (rep0/rep1 ride 1-2 bit codes).
            if len < 3 && !(len == 2 && i < 2) {
                continue;
            }
            let score = if self.h9_scoring {
                (7680i64 + i64::from(SHORT_CODE_DELTA[i]) + 540 * i64::from(len)) >> 2
            } else {
                // H5/H6: BackwardReferenceScoreUsingLastDistance
                // (135·len + 1935) − per-index penalty (0x1ca10-based)
                // for i > 0.
                let penalty = if i > 0 {
                    39 + ((0x1c_a10u64 >> (i & 0x0e)) & 0x0e) as i64
                } else {
                    0
                };
                135 * i64::from(len) + 1935 - penalty
            };
            if score > best_score as i64 {
                best_score = score as u64;
                best = Some((d, len));
            }
        }

        // Bucket scan, newest-first. The ring is walked as at most two
        // CONTIGUOUS slice segments (newest→0, mask→oldest) so the
        // per-entry loop runs on slice iterators without per-entry
        // bounds checks. The entry body is a macro: a capturing
        // closure defeated inlining and dominated the scan.
        macro_rules! consider {
            ($e:expr, $data:ident, $reject_limit:ident, $cur_best:ident, $pos_val:ident) => {{
                let prev = $e as usize;
                let backward = pos.wrapping_sub(prev);
                // Uninitialized slots are u32::MAX (backward huge);
                // skip them. Only break when a REAL position is beyond
                // the window — older ring entries are then also beyond.
                if prev != usize::MAX && backward != 0 {
                    if backward as u32 > self.max_distance {
                        break;
                    }
                    if prev < $reject_limit && $data[prev + $cur_best] != $pos_val {
                        continue;
                    }
                    // Upstream requires len >= 4 from the bucket scan
                    // (FindMatchLengthWithLimitMin4).
                    let len = self.match_len(pos, prev, max_len);
                    if len >= 4 {
                        let score = 7680u64
                            .wrapping_add(540u64.wrapping_mul(u64::from(len)))
                            .wrapping_sub(120u64
                                .wrapping_mul(u64::from(log2_floor(backward as u64))))
                            >> 2;
                        if score > best_score {
                            best_score = score;
                            best = Some((backward as u32, len));
                            let nb = len as usize;
                            if nb > $cur_best && pos + nb < $data.len() {
                                $cur_best = nb;
                                $pos_val = $data[pos + nb];
                            }
                        }
                    }
                }
            }};
        }

        let bank = 1usize << self.block_bits;
        let mask = bank - 1;
        let count = self.num[key] as usize;
        let base = key << self.block_bits;
        let bucket = &self.buckets[base..base + bank];
        let data = self.data;
        // Reject byte at the current best length, loaded once and
        // refreshed only when best changes (upstream prev_best_val).
        let mut cur_best = (best.map_or(3, |(_, l)| l)).max(min_len_hint) as usize;
        let mut pos_val = if pos + cur_best < data.len() {
            data[pos + cur_best]
        } else {
            0xFF
        };
        let reject_limit = data.len().saturating_sub(cur_best);

        let down = count.saturating_sub(bank);
        if count <= bank {
            // Ring not yet wrapped: entry i lives at slot i.
            for &e in bucket[down..count].iter().rev() {
                consider!(e, data, reject_limit, cur_best, pos_val);
            }
        } else {
            // Wrapped: full ring, two segments newest→0 and mask→oldest.
            let newest = (count - 1) & mask;
            for &e in bucket[0..=newest].iter().rev() {
                consider!(e, data, reject_limit, cur_best, pos_val);
            }
            for &e in bucket[newest + 1..].iter().rev() {
                consider!(e, data, reject_limit, cur_best, pos_val);
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
