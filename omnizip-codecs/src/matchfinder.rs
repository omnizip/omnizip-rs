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
        // dict_size; only the indexing space is rounded up. Inputs
        // smaller than the dictionary size the ring to the input —
        // chains can never reference before position 0 anyway, and the
        // smaller footprint keeps the walk cache-resident (measured
        // 2.8x -> 1x reference encode time on a 4 MiB fixture with an
        // 8 MiB dictionary).
        let prev_size = (dict_size as usize)
            .min(data.len().max(1024))
            .next_power_of_two();
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

    /// HC4-style chain walk that records the full improving-length
    /// candidate ladder (mirrors `hc_find_func` in xz's
    /// lz_encoder_mf.c): every candidate whose length strictly beats
    /// the best so far is appended as `(length, distance_1based)`.
    /// `start_len` seeds the best length (from the hash-2/hash-3
    /// pre-pass in the caller). Returns the final best length.
    ///
    /// `pos` must already have been inserted via `advance()`.
    pub fn walk_chain_ladder(
        &self,
        pos: usize,
        len_limit: u32,
        start_len: u32,
        out: &mut Vec<(u32, u32)>,
    ) -> u32 {
        if pos + 4 > self.data.len() {
            return start_len;
        }
        let h = Self::hash_n(self.data, pos, self.hash_log, self.hash_bytes);
        let mut candidate = self.head[h];
        if candidate == pos as u32 {
            candidate = self.prev[pos & self.mask as usize];
        }
        let mut len_best = start_len.max(1);
        let mut chain = 0u32;
        while candidate != SENTINEL && chain < self.max_chain_length {
            let cand_us = candidate as usize;
            let dist = pos.saturating_sub(cand_us);
            if dist == 0 || dist as u32 > self.max_distance {
                break;
            }

            // Quick reject: candidate can only improve if it matches
            // at the current best length (hc_find_func's
            // `pb[len_best] == cur[len_best] && pb[0] == cur[0]`).
            if len_best < len_limit
                && self.data[cand_us + len_best as usize] == self.data[pos + len_best as usize]
                && self.data[cand_us] == self.data[pos]
            {
                let max_len = len_limit.min((self.data.len() - pos) as u32);
                let len = Self::match_length(self.data, pos, cand_us, max_len);
                if len > len_best {
                    len_best = len;
                    out.push((len, dist as u32));
                    if len >= len_limit {
                        break;
                    }
                }
            }

            candidate = self.prev[cand_us & self.mask as usize];
            chain += 1;
        }
        len_best
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
        let n = self.data.len();
        self.store_and_find_capped(pos, n - pos, Some(out));
    }

    /// Upstream `Store`: insert + re-root with the compare capped at
    /// `TREE_MAX_COMP_LEN`, no matches returned. Used for the tail of
    /// long copies — a full-length compare there makes highly
    /// repetitive input quadratic (task #312: 1MB all-zeros at q11
    /// spent minutes in tail stores alone; the reference finishes in
    /// 0.07s).
    pub fn store(&mut self, pos: usize) {
        self.store_and_find_capped(pos, TREE_MAX_COMP_LEN, None);
    }

    /// Shared walk body. `cap_len` bounds the byte compare (upstream
    /// `max_length`; `Store` passes `MAX_TREE_COMP_LENGTH`), `out`
    /// receives matches when present (upstream `matches != NULL`).
    fn store_and_find_capped(
        &mut self,
        pos: usize,
        cap_len: usize,
        mut out: Option<&mut Vec<Lz77Match>>,
    ) {
        if let Some(o) = out.as_deref_mut() {
            o.clear();
        }
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
        let max_len = cap_len.min(n - pos);
        loop {
            let backward = pos.wrapping_sub(prev_ix);
            if prev_ix == self.invalid as usize || backward == 0 || backward > n || depth == 0 {
                self.forest[node_left] = self.invalid;
                self.forest[node_right] = self.invalid;
                break;
            }
            let cur_len = best_len_left.min(best_len_right);
            let len =
                cur_len + self.match_len_from(pos + cur_len, prev_ix + cur_len, max_len - cur_len);
            let best_so_far = out
                .as_deref()
                .and_then(|o| o.last().map(|m| m.length as usize))
                .unwrap_or(0);
            if len > best_so_far && len >= 4 {
                if let Some(o) = out.as_deref_mut() {
                    o.push(Lz77Match {
                        distance: backward as u32,
                        length: len as u32,
                    });
                }
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
    /// H68 tag bank: per-slot 8-bit hash tags.
    tags: Vec<u8>,
    /// H68 per-bucket DOWN-counter (0xFFFF − used), mirroring
    /// upstream's num_[] semantics for the head rotation math.
    tnum: Vec<u16>,
    use_tags: bool,
}

/// Upstream kMinScore = 30·8·8 + 100 (the baseline `out.score` that
/// every candidate must strictly beat to count as a found match).
const K_MIN_SCORE: u64 = 2020;

/// Refresh the reject-byte gate after a best-length change: a
/// macro so the reload inlines into the probe loops without a
/// capturing closure (which defeats inlining — measured).
macro_rules! gate_guard {
    ($gate_on:ident, $cur_best:ident, $data:ident, $pos:ident, $rep_val:ident) => {{
        $gate_on = $cur_best < $data.len() - $pos;
        $rep_val = if $gate_on {
            $data[$pos + $cur_best]
        } else {
            0xFF
        };
    }};
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
            h9_scoring: num_last_dists >= 16,
            hash5: false,
            tags: Vec::new(),
            tnum: Vec::new(),
            use_tags: false,
        }
    }

    /// Switch to the reference's SIMD-shape H68 tag-bank mode
    /// (hash_longest_match64_simd_inc.h): an 8-byte kHashMul64 hash
    /// split into a 15-bit key plus an 8-bit tag stored per entry.
    /// The scan compares the 16-entry tag array once and only visits
    /// tag-matching slots — the bitmask rejection is what makes the
    /// reference's q4-6 hashers several times faster than a full
    /// bank walk.
    pub fn enable_tag_mode(&mut self) {
        let bucket_count = 1usize << self.bucket_bits;
        self.tags = vec![0u8; bucket_count << self.block_bits];
        self.tnum = vec![0xFFFFu16; bucket_count];
        self.use_tags = true;
    }

    /// Switch to the H6 5-byte 64-bit hash (kHashMul64 << 24).
    pub fn enable_hash5(&mut self) {
        self.hash5 = true;
    }

    /// Positions a store/search needs within bounds: 8 for the H6
    /// 5-byte hash (upstream HashTypeLength), 4 otherwise.
    fn lookahead(&self) -> usize {
        if self.hash5 || self.use_tags {
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
        self.key_tag(pos).0
    }

    /// H68 8-byte hash: LOAD64LE * (kHashMul64 << 24) >> 41 gives 23
    /// bits: a 15-bit key (hash >> 8) plus an 8-bit tag (hash & 0xFF).
    #[inline(always)]
    fn key_tag(&self, pos: usize) -> (usize, u8) {
        let look = self.lookahead();
        if pos + look > self.data.len() {
            return (0, 0);
        }
        if self.use_tags {
            let d = &self.data[pos..pos + 8];
            let lo = u32::from_le_bytes([d[0], d[1], d[2], d[3]]);
            let hi = u32::from_le_bytes([d[4], d[5], d[6], d[7]]);
            let v = (u64::from(hi) << 32) | u64::from(lo);
            let v = v.wrapping_mul(0x1FE3_5A7B_D357_9BD3u64 << 24);
            let hash = (v >> 41) as usize;
            (hash >> 8, (hash & 0xFF) as u8)
        } else if self.hash5 {
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
            ((v >> (64 - self.bucket_bits)) as usize, 0)
        } else {
            let word = u32::from_le_bytes([
                self.data[pos],
                self.data[pos + 1],
                self.data[pos + 2],
                self.data[pos + 3],
            ]);
            (
                (word.wrapping_mul(0x1E35_A7BD) >> (32 - self.bucket_bits)) as usize,
                0,
            )
            // kHashMul32
        }
    }

    fn insert(&mut self, pos: usize) {
        if pos + self.lookahead() > self.data.len() {
            return;
        }
        let mask = (1usize << self.block_bits) - 1;
        if self.use_tags {
            let (key, tag) = self.key_tag(pos);
            let base = key << self.block_bits;
            let slot = usize::from(self.tnum[key]) & mask;
            self.buckets[base | slot] = pos as u32;
            self.tags[base | slot] = tag;
            self.tnum[key] = self.tnum[key].wrapping_sub(1);
            self.num[key] = self.num[key].wrapping_add(1);
            return;
        }
        let key = self.key(pos);
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
        if self.use_tags {
            let result = self.scan_with_tags(pos, last_dists, max_len, min_len_hint);
            if insert && pos < self.data.len() {
                self.insert(pos);
                if self.cur <= pos {
                    self.cur = pos + 1;
                }
            }
            return result;
        }
        let key = self.key(pos);
        // Prefetch-by-load (upstream PREFETCH_L1 on the bucket): the
        // bank is megabytes of randomly-accessed memory; issuing one
        // dependent-free word load here lets the miss overlap with the
        // rep probes inside the scan instead of serializing after
        // them. Measured ~3x per-find cost without it on text.
        std::hint::black_box(self.buckets[key << self.block_bits]);
        let result = self.scan_with_key(key, pos, last_dists, max_len, min_len_hint, u64::MAX);
        if insert && pos < self.data.len() {
            let mask = (1usize << self.block_bits) - 1;
            let base = key << self.block_bits;
            self.buckets[base | (self.num[key] as usize & mask)] = pos as u32;
            self.num[key] = self.num[key].wrapping_add(1);
            if self.cur <= pos {
                self.cur = pos + 1;
            }
        }
        result
    }

    fn match_len(&self, a: usize, b: usize, limit: u32) -> u32 {
        HashChainMatchFinder::match_length(self.data, a, b, limit)
    }

    /// Invariant fast path for the bank scan: callers guarantee
    /// `a + limit <= data.len()` and `b < a` (candidate positions are
    /// always older than the current one), so every byte access below
    /// is in bounds by construction — the generic `match_length`
    /// re-checks both bounds per step. Each window is sliced out ONCE
    /// (one bounds check per side) and the stepping runs on
    /// equal-length slice iterators with no per-step checks; indexing
    /// the data slice per step kept four panicking bounds checks per
    /// 8-byte step even after full inlining (LLVM cannot see the
    /// clamp through the u32 cast).
    #[inline(always)]
    fn match_len_scan(&self, a: usize, b: usize, limit: u32) -> u32 {
        // Upstream FindMatchLengthWithLimit shape: u32 first (short
        // rep matches — the common case — cost one load pair), then
        // u64 stepping. A u128 first-load was 2x slower on the
        // rep-probe-heavy text path (the 16-byte loads are wasted on
        // 2-8 byte matches).
        let data = self.data;
        let max = limit as usize;
        let sa = &data[a..a + max];
        let sb = &data[b..b + max];
        let mut len = 0usize;
        if max >= 4 {
            let wa = u32::from_le_bytes([sa[0], sa[1], sa[2], sa[3]]);
            let wb = u32::from_le_bytes([sb[0], sb[1], sb[2], sb[3]]);
            if wa != wb {
                return (wa ^ wb).trailing_zeros() / 8;
            }
            len = 4;
        }
        for (xa, xb) in sa[len..].chunks_exact(8).zip(sb[len..].chunks_exact(8)) {
            let wa = u64::from_le_bytes(xa.try_into().unwrap());
            let wb = u64::from_le_bytes(xb.try_into().unwrap());
            if wa == wb {
                len += 8;
            } else {
                return (len + (wa ^ wb).trailing_zeros() as usize / 8) as u32;
            }
        }
        for (&xa, &xb) in sa[len..].iter().zip(sb[len..].iter()) {
            if xa != xb {
                break;
            }
            len += 1;
        }
        len as u32
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
        if self.use_tags {
            return self.scan_with_tags(pos, last_dists, max_len, min_len_hint);
        }
        let key = self.key(pos);
        std::hint::black_box(self.buckets[key << self.block_bits]);
        self.scan_with_key(key, pos, last_dists, max_len, min_len_hint, u64::MAX)
    }

    /// Decision-only variant of [`find_with_floor`](Self::find_with_floor)
    /// for the lazy re-search: the scan aborts as soon as the best
    /// score reaches `stop_above`. When the caller only compares the
    /// returned score against `stop_above` (>=), the decision is
    /// identical to a full scan — an early return means the full scan
    /// would also have finished at or above the threshold, and without
    /// an early return the scan ran to completion. The returned
    /// distance/length are the abort-time best, NOT necessarily the
    /// full-scan best — do not use them for parse decisions.
    #[must_use]
    pub fn find_with_floor_stop(
        &self,
        pos: usize,
        last_dists: &[u32],
        max_len: u32,
        min_len_hint: u32,
        stop_above: u64,
    ) -> Option<(u32, u32, u64)> {
        if pos + self.lookahead() > self.data.len() || max_len < 2 {
            return None;
        }
        let key = self.key(pos);
        std::hint::black_box(self.buckets[key << self.block_bits]);
        self.scan_with_key(key, pos, last_dists, max_len, min_len_hint, stop_above)
    }

    /// H68 scan (hash_longest_match64_simd_inc.h FindLongestMatch):
    /// rep probes first (identical semantics to the plain scan), then
    /// ONE pass over the bucket's 16-entry tag array builds a bitmask
    /// and only tag-matching slots are visited — the bitmask rejection
    /// replaces the full bank walk.
    fn scan_with_tags(
        &self,
        pos: usize,
        last_dists: &[u32],
        max_len: u32,
        min_len_hint: u32,
    ) -> Option<(u32, u32, u64)> {
        const CACHE_INDEX: [u8; 16] = [0, 1, 2, 3, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1];
        const CACHE_OFFSET: [i8; 16] = [0, 0, 0, 0, -1, 1, -2, 2, -3, 3, -1, 1, -2, 2, -3, 3];
        const SHORT_CODE_DELTA: [i32; 16] = [
            60, -95, -117, -127, -93, -93, -96, -96, -99, -99, -105, -105, -115, -115, -125, -125,
        ];
        let mut best_score = K_MIN_SCORE;
        let mut best: Option<(u32, u32)> = None;
        let exact_only = self.num_last_dists <= 4;

        // Rep probes (same as scan_with_key).
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
            let len = self.match_len_scan(pos, prev, max_len);
            if len < 3 && !(len == 2 && i < 2) {
                continue;
            }
            let score = if self.h9_scoring {
                (7680i64 + i64::from(SHORT_CODE_DELTA[i]) + 540 * i64::from(len)) >> 2
            } else {
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

        // Tag-bank scan.
        let (key, tag) = self.key_tag(pos);
        let mask = (1usize << self.block_bits) - 1;
        let bank = 1usize << self.block_bits;
        let base = key << self.block_bits;
        let bucket = &self.buckets[base..base + bank];
        let tag_bucket = &self.tags[base..base + bank];
        let used = 65535usize.wrapping_sub(usize::from(self.tnum[key]));
        let head = (usize::from(self.tnum[key]) + 1) & mask;
        let mut cur_best = (best.map_or(3, |(_, l)| l)).max(min_len_hint) as usize;
        // Bitmask over slots relative to head (bit i = slot (head+i)&mask),
        // built with upstream's SWAR byte-equality gather
        // (matching_tag_mask.h, the non-SSE path): one XOR + subtract
        // per 8 tags, then a 16-bit rotate. This is the whole point of
        // the tag bank — the measured reference visits 0.38 entries
        // per find because the mask does all the rejection.
        let mut matches: u16;
        {
            const X01: u64 = 0x0101_0101_0101_0101;
            const X80: u64 = 0x8080_8080_8080_8080;
            let splat = u64::from(tag) * X01;
            let extract_magic = (u64::MAX / 0x7F) >> 8;
            let gather = |off: usize| -> u64 {
                let chunk =
                    u64::from_le_bytes(tag_bucket[off..off + 8].try_into().unwrap()) ^ splat;
                let zeros = ((chunk | X80).wrapping_sub(X01) | chunk) & X80;
                (zeros.wrapping_mul(extract_magic)) >> 56
            };
            // C build order: bytes 8-15 first, then bytes 0-7 shifted in.
            matches = (gather(8) as u16) << 8 | gather(0) as u16;
            matches = !matches;
            matches = matches.rotate_right(head as u32);
        }
        // Mask off uninitialized slots (upstream: n unused-entry guard).
        if bank > used {
            let used = used.min(16);
            matches &= ((1u16 << used) - 1) as u16;
        }
        while matches != 0 {
            let tz = matches.trailing_zeros() as usize;
            matches &= matches - 1;
            let prev = bucket[(head + tz) & mask] as usize;
            let backward = pos.wrapping_sub(prev);
            if backward == 0 || backward as u32 > self.max_distance {
                break;
            }
            // Double 4-byte reject (upstream): bytes ending at
            // cur_best+1 must match, then the first 4 bytes.
            let data = self.data;
            let bl = cur_best.max(3);
            if pos + bl + 1 > data.len() || prev + bl + 1 > data.len() {
                break;
            }
            let a = u32::from_le_bytes(data[pos + bl - 3..pos + bl + 1].try_into().unwrap());
            let b = u32::from_le_bytes(data[prev + bl - 3..prev + bl + 1].try_into().unwrap());
            if a != b {
                continue;
            }
            let f4a = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
            let f4b = u32::from_le_bytes(data[prev..prev + 4].try_into().unwrap());
            if f4a != f4b {
                continue;
            }
            let len4 = self.match_len_scan(pos + 4, prev + 4, max_len.saturating_sub(4)) + 4;
            if len4 >= 4 {
                let score = 7680u64
                    .wrapping_add(540u64.wrapping_mul(u64::from(len4)))
                    .wrapping_sub(120u64.wrapping_mul(u64::from(log2_floor(backward as u64))))
                    >> 2;
                if score > best_score {
                    best_score = score;
                    best = Some((backward as u32, len4));
                    let nb = len4 as usize;
                    if nb > cur_best {
                        cur_best = nb;
                    }
                }
            }
        }
        best.map(|(d, l)| (d, l, best_score))
    }

    // The H5/H6 rep scores are i64 by upstream shape and compared
    // against the u64 best_score; both are small positives, so the
    // cast cannot wrap. (Keeps clippy quiet without restructuring the
    // reference arithmetic.)
    #[allow(clippy::cast_possible_wrap)]
    fn scan_with_key(
        &self,
        key: usize,
        pos: usize,
        last_dists: &[u32],
        max_len: u32,
        min_len_hint: u32,
        stop_above: u64,
    ) -> Option<(u32, u32, u64)> {
        // match_len_scan assumes `pos + max_len <= data.len()`; clamp
        // once here so every caller shape satisfies it. With that
        // invariant, match_len_scan's slice accesses are in bounds by
        // construction (candidates are always older than `pos`), which
        // lets the inlined bounds checks fold away.
        let data = self.data;
        let max_len = max_len.min((data.len() - pos) as u32);
        // Upstream seeds the search result with kMinScore: a candidate
        // only "found" if it strictly beats it.
        let mut best_score = K_MIN_SCORE;
        // Early-abort ceiling for decision-only callers (the lazy
        // re-search): once best_score reaches it, no later candidate
        // can change the caller's >= comparison, so the scan stops.
        // u64::MAX (all full-scan callers) can never be reached.
        let mut best: Option<(u32, u32)> = None;
        // Reject gate for probes and scan entries: the byte at the
        // current best length (upstream prev_best_val shape). Starts at
        // 2 (the shortest candidate the rep codes can carry).
        let mut cur_best = 2usize;
        let ndists = self.num_last_dists.min(16);

        // Exact-rep-only mode for shallow configs (num_last_dists <= 4,
        // i.e. every text tier and binary q4-6): CACHE_INDEX[i] == i,
        // CACHE_OFFSET[i] == 0, and h9_scoring is necessarily false (it
        // requires num_last_dists >= 16) — probe the distance ring
        // directly. This is the q2-9 text hot path; the generic
        // 16-short-code loop below only serves the wide binary tiers.
        if self.num_last_dists <= 4 {
            // Reject byte at pos + cur_best, reloaded only when the
            // best length changes (upstream prev_best_val). When
            // cur_best runs past the window the gate is disabled,
            // matching the bounds-check fall-through. prev < pos, so
            // the single pos-side check subsumes the prev-side one.
            let mut gate_on = cur_best < data.len() - pos;
            let mut rep_val = if gate_on { data[pos + cur_best] } else { 0xFF };
            // Steady-state shape (ring full): unrolled four exact
            // probes with constant penalties — the steady state is
            // ~99% of calls, and the unrolled form drops the per-probe
            // ring bounds check and penalty arithmetic.
            let penalties = [
                0i64,
                39 + ((0x1c_a10u64) & 0x0e) as i64,
                39 + ((0x1c_a10u64 >> 2) & 0x0e) as i64,
                39 + ((0x1c_a10u64 >> 2) & 0x0e) as i64,
            ];
            for i in 0..ndists {
                let base = if i < last_dists.len() {
                    last_dists[i]
                } else {
                    0
                };
                let v = base as usize;
                if v == 0 || v > pos {
                    continue;
                }
                let prev = pos - v;
                // Skipping the compare when out of bounds matches the
                // generic path's behavior (fall through to the full
                // scan).
                if gate_on && rep_val != data[prev + cur_best] {
                    continue;
                }
                let len = self.match_len_scan(pos, prev, max_len);
                // Upstream: >= 3 always, len 2 only for the two exact-rep
                // codes (rep0/rep1 ride 1-2 bit codes).
                if len < 3 && !(len == 2 && i < 2) {
                    continue;
                }
                // H5/H6: BackwardReferenceScoreUsingLastDistance
                // (135·len + 1935) − per-index penalty (0x1ca10-based)
                // for i > 0.
                let score = 135 * i64::from(len) + 1935 - penalties[i];
                if score > best_score as i64 {
                    best_score = score as u64;
                    best = Some((base, len));
                    cur_best = len as usize;
                    gate_guard!(gate_on, cur_best, data, pos, rep_val);
                    if best_score >= stop_above {
                        return best.map(|(d, l)| (d, l, best_score));
                    }
                }
            }
        } else {
            // (distance-cache index, offset) per short code — upstream
            // kDistanceCacheIndex/kDistanceCacheOffset.
            const CACHE_INDEX: [u8; 16] = [0, 1, 2, 3, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1];
            const CACHE_OFFSET: [i8; 16] = [0, 0, 0, 0, -1, 1, -2, 2, -3, 3, -1, 1, -2, 2, -3, 3];
            // Short-code cost deltas from SCORE_BASE (upstream
            // kDistanceShortCodeCost); the H9 score is
            // (SCORE_BASE + delta + 540·len) >> 2.
            const SHORT_CODE_DELTA: [i32; 16] = [
                60, -95, -117, -127, -93, -93, -96, -96, -99, -99, -105, -105, -115, -115, -125,
                -125,
            ];
            for i in 0..ndists {
                let idx = usize::from(CACHE_INDEX[i]);
                let base = if idx < last_dists.len() {
                    last_dists[idx]
                } else {
                    0
                };
                let v = i64::from(base) + i64::from(CACHE_OFFSET[i]);
                if v < 1 || v as usize > pos {
                    continue;
                }
                let d = v as u32;
                let prev = pos - v as usize;
                if cur_best < data.len() - pos
                    && prev + cur_best < data.len()
                    && data[pos + cur_best] != data[prev + cur_best]
                {
                    continue;
                }
                let len = self.match_len_scan(pos, prev, max_len);
                // Upstream: >= 3 always, len 2 only for the two exact-rep
                // codes (rep0/rep1 ride 1-2 bit codes).
                if len < 3 && !(len == 2 && i < 2) {
                    continue;
                }
                let score = if self.h9_scoring {
                    (7680i64 + i64::from(SHORT_CODE_DELTA[i]) + 540 * i64::from(len)) >> 2
                } else {
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
                    cur_best = len as usize;
                    if best_score >= stop_above {
                        return best.map(|(d, l)| (d, l, best_score));
                    }
                }
            }
        }

        // Bucket scan, newest-first. One loop over the ring indices
        // (newest → oldest): before wrap the live slots are [0, count);
        // after wrap the slot for insert ordinal j is j & mask — the
        // same entry sequence the previous two-segment split visited,
        // in the same order.
        let bank = 1usize << self.block_bits;
        let mask = bank - 1;
        let count = self.num[key] as usize;
        let base = key << self.block_bits;
        let bucket = &self.buckets[base..base + bank];
        // Reject byte at the current best length, loaded once and
        // refreshed only when best changes (upstream prev_best_val).
        // Note the seed differs from the rep loop: no rep match means
        // no candidate below the scan's len-4 floor, so 3 (the exact
        // original seed).
        cur_best = best
            .map_or(3, |(_, l)| l as usize)
            .max(min_len_hint as usize);
        let mut pos_val = if pos + cur_best < data.len() {
            data[pos + cur_best]
        } else {
            0xFF
        };

        let down = count.saturating_sub(bank);
        let mut j = count;
        while j > down {
            j -= 1;
            let prev = bucket[j & mask] as usize;
            let backward = pos.wrapping_sub(prev);
            // Skip the self-entry (find_insert scans before
            // inserting, but the advance()+find() callers insert
            // first) and entries beyond the window — older ring
            // entries are then also beyond (arrival order,
            // scanned newest-first).
            if backward != 0 && backward as u32 <= self.max_distance {
                // In-bounds by construction: prev < pos and
                // pos + cur_best <= len (guarded via pos_val's load).
                if prev + cur_best < data.len() && data[prev + cur_best] != pos_val {
                    continue;
                }
                // Upstream requires len >= 4 from the bucket scan
                // (FindMatchLengthWithLimitMin4).
                let len = self.match_len_scan(pos, prev, max_len);
                if len >= 4 {
                    let score = 7680u64
                        .wrapping_add(540u64.wrapping_mul(u64::from(len)))
                        .wrapping_sub(120u64.wrapping_mul(u64::from(log2_floor(backward as u64))))
                        >> 2;
                    if score > best_score {
                        best_score = score;
                        best = Some((backward as u32, len));
                        let nb = len as usize;
                        if nb > cur_best && pos + nb < data.len() {
                            cur_best = nb;
                            pos_val = data[pos + nb];
                        }
                        if best_score >= stop_above {
                            return best.map(|(d, l)| (d, l, best_score));
                        }
                    }
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

/// Windowed BT4 match finder — port of xz's `lzma_mf_bt4_find` /
/// `lzma_mf_bt4_skip` (lz_encoder_mf.c). Positions sharing a 4-byte
/// hash bucket form a binary search tree keyed by byte comparison,
/// re-rooted at the current position on every find/skip; the walk
/// surfaces the best candidate at every length tier while pruning
/// whole subtrees, which is why the reference uses it for all NORMAL
/// presets. Unlike [`BinaryTreeMatchFinder`] (the brotli H10 port),
/// the forest is a cyclic array of `dict_size` slots, so memory is
/// bounded by the dictionary regardless of input length.
#[derive(Debug)]
pub struct Bt4MatchFinder<'a> {
    data: &'a [u8],
    /// Layout mirrors xz: [0..1024) = 2-byte hash, [1024..1024+65536)
    /// = 3-byte hash, rest = 4-byte hash.
    hash: Vec<u32>,
    hash4_offset: usize,
    hash4_mask: u32,
    /// Two u32 slots per cyclic position (left/right tree links).
    son: Vec<u32>,
    cyclic_mask: u32,
    cyclic_size: u32,
    depth: u32,
    nice_len: u32,
    /// CRC-32 low byte table (`lzma_crc32_table[0]` in xz) used by the
    /// hash mix.
    crc_table: [u32; 256],
}

const BT4_EMPTY: u32 = u32::MAX;
const HASH_2_SIZE: usize = 1 << 10;
const HASH_3_SIZE: usize = 1 << 16;

fn crc32_low_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    table
}

impl<'a> Bt4MatchFinder<'a> {
    /// Construct over `data` with a `dict_size`-slot cyclic window.
    /// `depth` 0 selects xz's auto depth (16 + nice_len / 2).
    #[must_use]
    pub fn new(data: &'a [u8], dict_size: u32, nice_len: u32, depth: u32) -> Self {
        let dict_size = dict_size.max(1024).next_power_of_two();
        let mut hs = dict_size - 1;
        hs |= hs >> 1;
        hs |= hs >> 2;
        hs |= hs >> 4;
        hs |= hs >> 8;
        hs >>= 1;
        hs |= 0xFFFF;
        if hs > (1 << 24) {
            hs >>= 1;
        }
        let hash4_size = (hs + 1) as usize;
        let nice_len = nice_len.clamp(4, 273);
        let depth = if depth == 0 { 16 + nice_len / 2 } else { depth };
        Self {
            data,
            hash: vec![BT4_EMPTY; HASH_2_SIZE + HASH_3_SIZE + hash4_size],
            hash4_offset: HASH_2_SIZE + HASH_3_SIZE,
            hash4_mask: hs,
            son: vec![BT4_EMPTY; dict_size as usize * 2],
            cyclic_mask: dict_size - 1,
            cyclic_size: dict_size,
            depth,
            nice_len,
            crc_table: crc32_low_table(),
        }
    }

    #[must_use]
    pub const fn depth(&self) -> u32 {
        self.depth
    }

    #[inline]
    fn hash_indexes(&self, pos: usize) -> (usize, usize, usize) {
        let d = self.data;
        let temp = self.crc_table[d[pos] as usize] ^ u32::from(d[pos + 1]);
        let h3 = (temp ^ (u32::from(d[pos + 2]) << 8)) & (HASH_3_SIZE as u32 - 1);
        let h4 = (temp ^ (u32::from(d[pos + 2]) << 8) ^ (self.crc_table[d[pos + 3] as usize] << 5))
            & self.hash4_mask;
        (
            (temp & (HASH_2_SIZE as u32 - 1)) as usize,
            (HASH_2_SIZE + h3 as usize),
            self.hash4_offset + h4 as usize,
        )
    }

    #[inline]
    fn match_len(&self, a: usize, b: usize, start: u32, limit: u32) -> u32 {
        let d = self.data;
        let mut len = start as usize;
        let limit = limit as usize;
        // 16-byte chunks first — two loads per iteration like the
        // reference's word-wide memcmplen (u64 stepping costs twice
        // the loads and dominated the skip path in profiles).
        while len + 16 <= limit {
            let wa = u128::from_le_bytes(d[len + a..len + a + 16].try_into().expect("16"));
            let wb = u128::from_le_bytes(d[len + b..len + b + 16].try_into().expect("16"));
            if wa == wb {
                len += 16;
            } else {
                return (len + (wa ^ wb).trailing_zeros() as usize / 8) as u32;
            }
        }
        while len + 8 <= limit {
            let wa = u64::from_le_bytes(d[len + a..len + a + 8].try_into().expect("8"));
            let wb = u64::from_le_bytes(d[len + b..len + b + 8].try_into().expect("8"));
            if wa == wb {
                len += 8;
            } else {
                return (len + (wa ^ wb).trailing_zeros() as usize / 8) as u32;
            }
        }
        while len < limit && d[a + len] == d[b + len] {
            len += 1;
        }
        len as u32
    }

    /// Port of `lzma_mf_bt4_find`: insert `pos` and walk its tree,
    /// appending the improving-length ladder as `(length, dist0)`
    /// pairs. Returns the longest length.
    pub fn find(&mut self, pos: usize, out: &mut Vec<(u32, u32)>) -> u32 {
        let avail = (self.data.len() - pos) as u32;
        let len_limit = self.nice_len.min(avail);

        let (h2, h3, h4) = self.hash_indexes(pos);
        let old2 = self.hash[h2];
        let old3 = self.hash[h3];
        let cur_match = self.hash[h4];
        self.hash[h2] = pos as u32;
        self.hash[h3] = pos as u32;
        self.hash[h4] = pos as u32;

        let valid = |old: u32, pos: usize| {
            old != BT4_EMPTY && old < pos as u32 && (pos as u32 - old) < self.cyclic_size
        };

        let mut delta2 = if valid(old2, pos) {
            pos as u32 - old2
        } else {
            0
        };
        let delta3 = if valid(old3, pos) {
            pos as u32 - old3
        } else {
            0
        };
        let mut len_best = 1u32;

        if delta2 != 0 && self.data[pos - delta2 as usize] == self.data[pos] {
            len_best = 2;
            out.push((2, delta2 - 1));
        }
        if delta3 != 0 && delta3 != delta2 && self.data[pos - delta3 as usize] == self.data[pos] {
            len_best = 3;
            out.push((3, delta3 - 1));
            delta2 = delta3;
        }
        if let Some(last) = out.last_mut() {
            len_best = self.match_len(pos, pos - delta2 as usize, len_best, len_limit);
            last.0 = len_best;
            if len_best == len_limit {
                self.tree_walk(pos, cur_match, len_limit, len_best, None);
                return len_best;
            }
        }
        if len_best < 3 {
            len_best = 3;
        }
        self.tree_walk(pos, cur_match, len_limit, len_best, Some(out));
        out.last().map_or(len_best, |m| m.0.max(len_best))
    }

    /// Port of `lzma_mf_bt4_skip`: insert `pos` without reporting
    /// matches.
    pub fn skip(&mut self, pos: usize) {
        if pos + 4 > self.data.len() {
            return;
        }
        let avail = (self.data.len() - pos) as u32;
        let len_limit = self.nice_len.min(avail);
        let (h2, h3, h4) = self.hash_indexes(pos);
        let cur_match = self.hash[h4];
        self.hash[h2] = pos as u32;
        self.hash[h3] = pos as u32;
        self.hash[h4] = pos as u32;
        self.skip_walk(pos, cur_match, len_limit);
    }

    /// `bt_skip_func` — the tree walk without match recording. Split
    /// from `tree_walk` so the skip hot path (greedy parses over
    /// repetitive data skip most positions) compiles without the
    /// recording machinery.
    fn skip_walk(&mut self, pos: usize, cur_match_in: u32, len_limit: u32) {
        let cyclic_pos = (pos as u32) & self.cyclic_mask;
        let mut ptr0 = ((cyclic_pos << 1) + 1) as usize;
        let mut ptr1 = (cyclic_pos << 1) as usize;
        let mut len0 = 0u32;
        let mut len1 = 0u32;
        let mut cur_match = cur_match_in;
        let mut depth_left = self.depth;
        let empty = BT4_EMPTY;

        loop {
            let delta = (pos as u32).wrapping_sub(cur_match);
            if depth_left == 0 || cur_match == empty || delta >= self.cyclic_size {
                self.son[ptr0] = empty;
                self.son[ptr1] = empty;
                return;
            }
            depth_left -= 1;

            let pair = (((cyclic_pos.wrapping_sub(delta)) & self.cyclic_mask) << 1) as usize;
            let pb = pos - delta as usize;
            let mut len = len0.min(len1);

            if len < len_limit && self.data[pb + len as usize] == self.data[pos + len as usize] {
                len = self.match_len(pb, pos, len + 1, len_limit);
                if len == len_limit {
                    self.son[ptr1] = self.son[pair];
                    self.son[ptr0] = self.son[pair + 1];
                    return;
                }
            }

            if len < len_limit && self.data[pb + len as usize] < self.data[pos + len as usize] {
                self.son[ptr1] = cur_match;
                ptr1 = pair + 1;
                cur_match = self.son[ptr1];
                len1 = len;
            } else {
                self.son[ptr0] = cur_match;
                ptr0 = pair;
                cur_match = self.son[ptr0];
                len0 = len;
            }
        }
    }

    /// Shared body of `bt_find_func` / `bt_skip_func`. `out` None =
    /// skip mode.
    fn tree_walk(
        &mut self,
        pos: usize,
        cur_match_in: u32,
        len_limit: u32,
        len_best_in: u32,
        mut out: Option<&mut Vec<(u32, u32)>>,
    ) {
        let cyclic_pos = (pos as u32) & self.cyclic_mask;
        let mut ptr0 = ((cyclic_pos << 1) + 1) as usize;
        let mut ptr1 = (cyclic_pos << 1) as usize;
        let mut len0 = 0u32;
        let mut len1 = 0u32;
        let mut cur_match = cur_match_in;
        let mut len_best = len_best_in;
        let mut depth_left = self.depth;
        let empty = BT4_EMPTY;

        loop {
            let delta = (pos as u32).wrapping_sub(cur_match);
            if depth_left == 0 || delta >= self.cyclic_size || cur_match == empty {
                self.son[ptr0] = empty;
                self.son[ptr1] = empty;
                return;
            }
            depth_left -= 1;

            let pair_base = ((cyclic_pos.wrapping_sub(delta)) & self.cyclic_mask) << 1;
            let pair = pair_base as usize;
            let pb = pos - delta as usize;
            let mut len = len0.min(len1);

            // The C reads pb[len]/cur[len] one past len_limit into the
            // dictionary buffer's slack; at len == len_limit the walk
            // cannot extend or record anything (len_best >= len_limit
            // already holds there), so the comparison is only needed
            // while len < len_limit.
            if len < len_limit && self.data[pb + len as usize] == self.data[pos + len as usize] {
                len = self.match_len(pb, pos, len + 1, len_limit);
                if len_best < len {
                    len_best = len;
                    if let Some(o) = out.as_deref_mut() {
                        o.push((len, delta - 1));
                    }
                    if len == len_limit {
                        let p1 = self.son[pair];
                        let p0 = self.son[pair + 1];
                        self.son[ptr1] = p1;
                        self.son[ptr0] = p0;
                        return;
                    }
                }
            }

            if len < len_limit && self.data[pb + len as usize] < self.data[pos + len as usize] {
                self.son[ptr1] = cur_match;
                ptr1 = pair + 1;
                cur_match = self.son[ptr1];
                len1 = len;
            } else {
                self.son[ptr0] = cur_match;
                ptr0 = pair;
                cur_match = self.son[ptr0];
                len0 = len;
            }
        }
    }
}

#[cfg(test)]
mod bt4_tests {
    use super::Bt4MatchFinder;

    #[test]
    fn finds_repeated_phrase() {
        let data = b"abcdefgh abcdefgh abcdefgh".as_slice();
        let mut mf = Bt4MatchFinder::new(data, 1 << 16, 64, 0);
        let mut out = Vec::new();
        let mut best = 0;
        for pos in 0..data.len().saturating_sub(4) {
            out.clear();
            best = best.max(mf.find(pos, &mut out));
            for &(len, dist) in &out {
                let back = pos - dist as usize - 1;
                assert_eq!(
                    &data[pos..pos + len as usize],
                    &data[back..back + len as usize]
                );
            }
        }
        assert!(best >= 8, "expected the 9-byte repeats, got {best}");
    }

    #[test]
    fn skip_keeps_tree_valid() {
        let data = b"the quick brown fox jumps over the lazy dog the quick brown fox".as_slice();
        let mut mf = Bt4MatchFinder::new(data, 1 << 16, 64, 0);
        for pos in 0..20 {
            mf.skip(pos);
        }
        let mut out = Vec::new();
        let best = mf.find(45, &mut out);
        assert!(best >= 4, "expected 'quick' era match, got {best}");
    }

    #[test]
    fn windowed_positions_expire() {
        // With a tiny window, candidates beyond dict_size must never
        // be returned.
        let mut data = vec![0u8; 4096];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let dict = 512u32;
        let mut mf = Bt4MatchFinder::new(&data, dict, 64, 0);
        let mut out = Vec::new();
        for pos in 0..data.len() - 4 {
            out.clear();
            mf.find(pos, &mut out);
            for &(_, dist) in &out {
                assert!(dist < dict, "dist {dist} beyond window {dict}");
            }
        }
    }
}
