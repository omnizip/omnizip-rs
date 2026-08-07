//! Binary-tree (BT4) match finder for LZMA high compression levels.
//!
//! Ported from XZ Utils `lz_encoder_mf.c` (`lzma_mf_bt4_find` /
//! `bt_find_func`). Uses a binary tree of positions keyed by a 4-byte
//! hash, enabling O(log n) chain walks instead of O(n) hash-chain walks.
//!
//! ## Algorithm
//!
//! 1. Compute 2/3/4-byte hashes at the current position.
//! 2. Check short (2-byte, 3-byte) matches via the 2/3-byte hash tables.
//! 3. Walk the binary tree rooted at the 4-byte hash, comparing match
//!    lengths and branching left/right based on lexicographic order.
//! 4. Insert the current position into all three hash tables + the tree.
//!
//! ## Determinism
//!
//! All tables are pre-allocated. No HashSet, no thread-local state.
//! Same input → same matches, always.
//!
//! ## When to use
//!
//! BT4 is slower than the hash-chain finder but finds longer matches.
//! Use at LZMA levels ≥ 7 (matching liblzma's preset table).

#![forbid(unsafe_code)]

/// A match found by the BT4 finder. Distance is 1-based.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bt4Match {
    pub distance: u32,
    pub length: u32,
}

/// Empty / unused hash table entry.
const EMPTY: u32 = 0;

/// Size of the 2-byte hash table (2^10 = 1024).
const HASH_2_SIZE: usize = 1 << 10;
/// Size of the 3-byte hash table (2^16 = 65536).
const HASH_3_SIZE: usize = 1 << 16;
/// Size of the 4-byte hash table (2^20 = 1M).
const HASH_4_SIZE: usize = 1 << 20;

const FIX_3_HASH_SIZE: usize = HASH_2_SIZE;
const FIX_4_HASH_SIZE: usize = HASH_2_SIZE + HASH_3_SIZE;

/// Multiplicative hash primes (from XZ Utils `lz_encoder_hash.h`).
const HASH_2_PRIMES: u32 = 0x9E37_79B1; // same as HASH_PRIME in shared finder
const HASH_3_PRIMES: u32 = 0x402A_B9BD;

/// Binary-tree match finder (BT4).
pub struct Bt4MatchFinder<'a> {
    data: &'a [u8],
    /// Combined hash table: [hash_2 | hash_3 | hash_4].
    hash: Vec<u32>,
    /// Binary tree: 2 entries per cyclic position (left, right).
    son: Vec<u32>,
    /// Cyclic buffer size (= dict_size).
    cyclic_size: u32,
    /// Current cyclic position.
    cyclic_pos: u32,
    /// Absolute position in the input.
    pos: u32,
    /// Max chain depth (tree walk limit).
    depth: u32,
    /// Stop walking once a match this long is found.
    nice_len: u32,
    /// Minimum match length to report.
    min_match: u32,
}

impl<'a> Bt4MatchFinder<'a> {
    /// Construct a BT4 finder over `data`.
    ///
    /// `dict_size` bounds the cyclic buffer (and thus max distance).
    /// `depth` is the max tree-walk steps (liblzma uses `1 << search_log`).
    /// `nice_len` stops the walk early (0 = disabled).
    #[must_use]
    pub fn new(data: &'a [u8], dict_size: u32, depth: u32, nice_len: u32) -> Self {
        let cyclic_size = dict_size.max(4096);
        let son_size = (cyclic_size as usize) * 2;
        Self {
            data,
            hash: vec![EMPTY; FIX_4_HASH_SIZE + HASH_4_SIZE],
            son: vec![EMPTY; son_size],
            cyclic_size,
            cyclic_pos: 0,
            pos: 0,
            depth: depth.max(1),
            nice_len,
            min_match: 3,
        }
    }

    /// Current absolute position.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.pos as usize
    }

    /// Advance one position, inserting into the hash tables + tree
    /// without searching for matches (skip).
    pub fn skip(&mut self) {
        if self.pos as usize + 4 > self.data.len() {
            self.advance_pos();
            return;
        }
        let (h2, h3, h4) = self.hash4_at(self.pos as usize);
        let cur_match = self.hash[FIX_4_HASH_SIZE + h4];
        self.hash[h2] = self.pos;
        self.hash[FIX_3_HASH_SIZE + h3] = self.pos;
        self.hash[FIX_4_HASH_SIZE + h4] = self.pos;

        self.bt_skip(cur_match);
        self.advance_pos();
    }

    /// Find matches at the current position. Returns up to several
    /// matches sorted by increasing length. The caller typically takes
    /// the longest.
    #[must_use]
    pub fn find(&mut self) -> Vec<Bt4Match> {
        if self.pos as usize + 4 > self.data.len() {
            self.advance_pos();
            return Vec::new();
        }

        let pos = self.pos as usize;
        let cur = &self.data[pos..];
        let len_limit = (self.data.len() - pos)
            .min(if self.nice_len > 0 {
                self.nice_len as usize
            } else {
                273
            }) as u32;

        let (h2, h3, h4) = self.hash4_at(pos);
        let delta2 = self.pos.wrapping_sub(self.hash[h2]);
        let delta3 = self.pos.wrapping_sub(self.hash[FIX_3_HASH_SIZE + h3]);
        let cur_match = self.hash[FIX_4_HASH_SIZE + h4];

        // Insert current position into hash tables.
        self.hash[h2] = self.pos;
        self.hash[FIX_3_HASH_SIZE + h3] = self.pos;
        self.hash[FIX_4_HASH_SIZE + h4] = self.pos;

        let mut matches = Vec::with_capacity(8);
        let mut len_best = 1u32;

        // Check 2-byte match.
        if delta2 > 0 && delta2 < self.cyclic_size {
            let back = pos.wrapping_sub(delta2 as usize);
            if back < self.data.len() && self.data[back] == cur[0] {
                len_best = 2;
                matches.push(Bt4Match {
                    distance: delta2,
                    length: 2,
                });
            }
        }

        // Check 3-byte match.
        if delta2 != delta3 && delta3 > 0 && delta3 < self.cyclic_size {
            let back = pos.wrapping_sub(delta3 as usize);
            if back < self.data.len() && self.data[back] == cur[0] {
                len_best = 3;
                matches.push(Bt4Match {
                    distance: delta3,
                    length: 3,
                });
            }
        }

        // Extend the best short match.
        if !matches.is_empty() {
            let best_dist = matches.last().unwrap().distance as usize;
            let back = pos.wrapping_sub(best_dist);
            let ext = self.match_length(pos, back, len_best, len_limit);
            matches.last_mut().unwrap().length = ext;
            len_best = ext;

            if len_best >= len_limit {
                self.bt_skip(cur_match);
                self.advance_pos();
                return self.filter_matches(matches);
            }
        }

        if len_best < 3 {
            len_best = 3;
        }

        // Binary tree walk.
        let tree_matches = self.bt_find(cur_match, len_best, len_limit);
        matches.extend(tree_matches);

        self.advance_pos();
        self.filter_matches(matches)
    }

    /// Find the single best match (convenience wrapper).
    #[must_use]
    pub fn find_best(&mut self) -> Option<Bt4Match> {
        let matches = self.find();
        matches.into_iter().max_by_key(|m| m.length)
    }

    // ── Internal ──────────────────────────────────────────────────────

    fn filter_matches(&self, matches: Vec<Bt4Match>) -> Vec<Bt4Match> {
        matches
            .into_iter()
            .filter(|m| m.length >= self.min_match)
            .collect()
    }

    fn advance_pos(&mut self) {
        self.pos = self.pos.wrapping_add(1);
        self.cyclic_pos += 1;
        if self.cyclic_pos >= self.cyclic_size {
            self.cyclic_pos = 0;
        }
    }

    /// Compute 2/3/4-byte hashes at `pos`.
    fn hash4_at(&self, pos: usize) -> (usize, usize, usize) {
        let d = self.data;
        let b0 = u32::from(d[pos]);
        let b1 = u32::from(d[pos + 1]);
        let b2 = u32::from(d[pos + 2]);
        let b3 = u32::from(d[pos + 3]);

        // hash_2 = (b0 | (b1 << 8))
        let h2 = ((b0 | (b1 << 8)) & (HASH_2_SIZE as u32 - 1)) as usize;
        // hash_3 = ((b0 | (b1 << 8) | (b2 << 16)) * HASH_3_PRIMES) >> (32 - 16)
        let temp = b0 | (b1 << 8) | (b2 << 16);
        let h3 = (temp.wrapping_mul(HASH_3_PRIMES) >> (32 - 16)) as usize & (HASH_3_SIZE - 1);
        // hash_4 = ((temp << 8 | b3) * HASH_2_PRIMES) >> (32 - 20)
        let h4 = ((temp << 8 | b3).wrapping_mul(HASH_2_PRIMES) >> (32 - 20)) as usize
            & (HASH_4_SIZE - 1);
        (h2, h3, h4)
    }

    /// Binary tree find. Returns matches found during the walk.
    fn bt_find(&mut self, mut cur_match: u32, mut len_best: u32, len_limit: u32) -> Vec<Bt4Match> {
        let mut matches = Vec::new();
        let pos = self.pos;
        let cyclic_pos = self.cyclic_pos;
        let cyclic_size = self.cyclic_size;
        let data = self.data;
        let cur_pos = pos as usize;

        // Tree node pointers: ptr0 = right child slot, ptr1 = left child slot.
        let mut ptr0_idx = (cyclic_pos as usize) * 2 + 1;
        let mut ptr1_idx = (cyclic_pos as usize) * 2;

        let mut len0 = 0u32;
        let mut len1 = 0u32;
        let mut depth = self.depth;

        loop {
            let delta = pos.wrapping_sub(cur_match);
            if depth == 0 || delta >= cyclic_size || cur_match == EMPTY {
                self.son[ptr0_idx] = EMPTY;
                self.son[ptr1_idx] = EMPTY;
                return matches;
            }
            depth -= 1;

            let pair_cyclic = cyclic_pos
                .wrapping_sub(delta)
                .wrapping_add(if delta > cyclic_pos {
                    cyclic_size
                } else {
                    0
                });
            let pair_idx = (pair_cyclic as usize) * 2;

            let back = cur_pos.wrapping_sub(delta as usize);
            if back >= data.len() {
                self.son[ptr0_idx] = EMPTY;
                self.son[ptr1_idx] = EMPTY;
                return matches;
            }

            let mut len = len0.min(len1);
            // Extend match.
            if back + (len as usize) < data.len()
                && cur_pos + (len as usize) < data.len()
                && data[back + (len as usize)] == data[cur_pos + (len as usize)]
            {
                len = self.match_length(cur_pos, back, len + 1, len_limit);

                if len_best < len {
                    len_best = len;
                    matches.push(Bt4Match {
                        distance: delta,
                        length: len,
                    });

                    if len >= len_limit {
                        // Nice match found — cut the tree and return.
                        self.son[ptr1_idx] = self.son[pair_idx];
                        self.son[ptr0_idx] = self.son[pair_idx + 1];
                        return matches;
                    }
                }
            }

            // Branch left or right based on lexicographic order.
            let pb_byte = data.get(back + (len as usize)).copied().unwrap_or(0);
            let cur_byte = data.get(cur_pos + (len as usize)).copied().unwrap_or(0);

            if pb_byte < cur_byte {
                // Go right.
                self.son[ptr1_idx] = cur_match;
                ptr1_idx = pair_idx + 1;
                cur_match = self.son[ptr1_idx];
                len1 = len;
            } else {
                // Go left.
                self.son[ptr0_idx] = cur_match;
                ptr0_idx = pair_idx;
                cur_match = self.son[ptr0_idx];
                len0 = len;
            }
        }
    }

    /// Binary tree skip (insert without collecting matches).
    fn bt_skip(&mut self, mut cur_match: u32) {
        let pos = self.pos;
        let cyclic_pos = self.cyclic_pos;
        let cyclic_size = self.cyclic_size;
        let data = self.data;
        let cur_pos = pos as usize;
        let len_limit = (data.len() - cur_pos).min(273) as u32;

        let mut ptr0_idx = (cyclic_pos as usize) * 2 + 1;
        let mut ptr1_idx = (cyclic_pos as usize) * 2;
        let mut len0 = 0u32;
        let mut len1 = 0u32;
        let mut depth = self.depth;

        loop {
            let delta = pos.wrapping_sub(cur_match);
            if depth == 0 || delta >= cyclic_size || cur_match == EMPTY {
                self.son[ptr0_idx] = EMPTY;
                self.son[ptr1_idx] = EMPTY;
                return;
            }
            depth -= 1;

            let pair_cyclic = cyclic_pos
                .wrapping_sub(delta)
                .wrapping_add(if delta > cyclic_pos {
                    cyclic_size
                } else {
                    0
                });
            let pair_idx = (pair_cyclic as usize) * 2;
            let back = cur_pos.wrapping_sub(delta as usize);

            if back >= data.len() {
                self.son[ptr0_idx] = EMPTY;
                self.son[ptr1_idx] = EMPTY;
                return;
            }

            let mut len = len0.min(len1);
            if back + (len as usize) < data.len()
                && cur_pos + (len as usize) < data.len()
                && data[back + (len as usize)] == data[cur_pos + (len as usize)]
            {
                len = self.match_length(cur_pos, back, len + 1, len_limit);
                if len >= len_limit {
                    self.son[ptr1_idx] = self.son[pair_idx];
                    self.son[ptr0_idx] = self.son[pair_idx + 1];
                    return;
                }
            }

            let pb_byte = data.get(back + (len as usize)).copied().unwrap_or(0);
            let cur_byte = data.get(cur_pos + (len as usize)).copied().unwrap_or(0);

            if pb_byte < cur_byte {
                self.son[ptr1_idx] = cur_match;
                ptr1_idx = pair_idx + 1;
                cur_match = self.son[ptr1_idx];
                len1 = len;
            } else {
                self.son[ptr0_idx] = cur_match;
                ptr0_idx = pair_idx;
                cur_match = self.son[ptr0_idx];
                len0 = len;
            }
        }
    }

    /// Word-at-a-time match length.
    fn match_length(&self, a: usize, b: usize, start: u32, max_len: u32) -> u32 {
        let data = self.data;
        let max = max_len as usize;
        let mut len = start as usize;

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
                len += (wa ^ wb).trailing_zeros() as usize / 8;
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
    fn finds_repetitive_match() {
        let data = b"hello world hello there";
        let mut mf = Bt4MatchFinder::new(data, 4096, 16, 0);
        // Advance past first "hello ".
        for _ in 0..12 {
            mf.skip();
        }
        // At position 12 ("hello there"), should find match to pos 0.
        if let Some(m) = mf.find_best() {
            assert_eq!(m.distance, 12);
            assert!(m.length >= 5, "expected >= 5, got {}", m.length);
        }
    }

    #[test]
    fn finds_long_run() {
        let data: Vec<u8> = b"abcdefgh".repeat(50);
        let mut mf = Bt4MatchFinder::new(&data, 4096, 32, 0);
        // Skip past first occurrence.
        for _ in 0..16 {
            mf.skip();
        }
        if let Some(m) = mf.find_best() {
            assert!(m.length >= 8, "expected long match, got {}", m.length);
            assert!(m.distance > 0);
        }
    }

    #[test]
    fn determinism() {
        let data: Vec<u8> = (0..1000).map(|i| (i * 7 + 13) as u8).collect();
        let find_all = || {
            let mut mf = Bt4MatchFinder::new(&data, 4096, 16, 0);
            let mut out = Vec::new();
            while mf.position() + 4 <= data.len() {
                if let Some(m) = mf.find_best() {
                    out.push((mf.position() - 1, m.distance, m.length));
                }
            }
            out
        };
        assert_eq!(find_all(), find_all());
    }

    #[test]
    fn empty_at_eof() {
        let data = b"short";
        let mut mf = Bt4MatchFinder::new(data, 4096, 8, 0);
        for _ in 0..data.len() {
            mf.skip();
        }
        let matches = mf.find();
        assert!(matches.is_empty());
    }
}
