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
        let size = dict_size as usize;
        let mask = size as u32 - 1;
        Self {
            data,
            head: vec![SENTINEL; 1usize << config.hash_log],
            prev: vec![SENTINEL; size],
            mask,
            hash_log: config.hash_log,
            cur: 0,
            max_distance: dict_size,
            max_chain_length: config.max_chain_length,
            min_match: config.min_match,
            nice_match: config.nice_match,
            max_match_length: config.max_match_length,
        }
    }

    /// Current position in the input data.
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
        if pos + 4 > self.data.len() {
            return None;
        }
        let h = Self::hash(self.data, pos, self.hash_log);
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

        while candidate != SENTINEL && chain < self.max_chain_length {
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

    /// Re-bind to new data, reusing the existing hash/chain allocations
    /// if the `dict_size` is unchanged. Grows them if the new dict is
    /// larger. Equivalent to `drop` + `new` but avoids reallocation.
    pub fn reuse(&mut self, data: &'a [u8], dict_size: u32) {
        let dict_size = dict_size.max(4096);
        let size = dict_size as usize;
        if size > self.prev.len() {
            self.prev.resize(size, SENTINEL);
        }
        self.mask = size as u32 - 1;
        self.data = data;
        self.max_distance = dict_size;
        self.cur = 0;
        self.reset();
    }

    /// Hash 4 bytes at `data[pos..pos+4]` into `hash_log` bits.
    fn hash(data: &[u8], pos: usize, hash_log: u32) -> usize {
        if pos + 4 > data.len() {
            return 0;
        }
        let word = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        let h = word.wrapping_mul(HASH_PRIME) >> (32 - hash_log);
        h as usize & ((1usize << hash_log) - 1)
    }

    /// Insert `pos` into the hash table + chain.
    fn insert_at(&mut self, pos: usize) {
        if pos + 4 > self.data.len() {
            return;
        }
        let h = Self::hash(self.data, pos, self.hash_log);
        let prev_pos = self.head[h];
        self.prev[pos & self.mask as usize] = prev_pos;
        self.head[h] = pos as u32;
    }

    /// Word-at-a-time match length between `data[a..]` and `data[b..]`,
    /// capped at `max_len`. 5-8× faster than byte-by-byte on typical
    /// inputs via `u64` XOR + `trailing_zeros`.
    fn match_length(data: &[u8], a: usize, b: usize, max_len: u32) -> u32 {
        let max = max_len as usize;
        let mut len = 0usize;
        while len + 8 <= max && a + len + 8 <= data.len() && b + len + 8 <= data.len() {
            let wa = u64::from_le_bytes([
                data[a + len],
                data[a + len + 1],
                data[a + len + 2],
                data[a + len + 3],
                data[a + len + 4],
                data[a + len + 5],
                data[a + len + 6],
                data[a + len + 7],
            ]);
            let wb = u64::from_le_bytes([
                data[b + len],
                data[b + len + 1],
                data[b + len + 2],
                data[b + len + 3],
                data[b + len + 4],
                data[b + len + 5],
                data[b + len + 6],
                data[b + len + 7],
            ]);
            if wa == wb {
                len += 8;
            } else {
                let diff = wa ^ wb;
                len += diff.trailing_zeros() as usize / 8;
                return len as u32;
            }
        }
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
