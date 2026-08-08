//! Long-Distance Matching (LDM) for ZSTD.
//!
//! LDM uses a sparse hash table to find matches at very large distances
//! (beyond the normal window). This dramatically improves ratio on large
//! files with repeated blocks.
//!
//! The C reference enables LDM at levels ≥ 19 and for all levels when
//! `--long` is specified. Our implementation is simpler: a sparse hash
//! table with configurable coverage.
//!
//! ## Algorithm
//!
//! 1. Hash every `ldm_gap`-th position (default 64 bytes) using a
//!    separate hash table with `ldm_hash_log` bits.
//! 2. At each match position, look up the LDM hash to find candidates
//!    from earlier in the input (potentially beyond the normal window).
//! 3. Extend the match and compare length with the normal LZ77 match.
//! 4. Prefer the longer match.

#![forbid(unsafe_code)]

/// Sparse hash table for long-distance matching.
pub struct LdmHashTable {
    /// Hash table: head[hash] = most recent position with this hash.
    head: Vec<u32>,
    /// Chain: prev[pos / ldm_gap] = previous position with same hash.
    chain: Vec<u32>,
    /// Number of hash bits.
    hash_log: u32,
    /// Sample every N-th position (sparse sampling for memory control).
    gap: usize,
}

/// An LDM match result.
#[derive(Clone, Copy, Debug)]
pub struct LdmMatch {
    pub distance: u32,
    pub length: u32,
}

impl LdmHashTable {
    /// Create a new LDM hash table for an input of `src_size` bytes.
    ///
    /// - `window_log`: the ZSTD window size (controls hash table coverage).
    /// - `gap`: sample every N-th position (default 64 for sparse coverage).
    pub fn new(window_log: u32, gap: usize) -> Self {
        let hash_log = window_log.min(21); // cap at 2M entries
        let hash_size = 1usize << hash_log;
        let chain_size = (1usize << window_log) / gap.max(1);
        Self {
            head: vec![u32::MAX; hash_size],
            chain: vec![u32::MAX; chain_size.max(1)],
            hash_log,
            gap: gap.max(1),
        }
    }

    /// Compute the hash of 4 bytes at `pos` in `data`.
    fn hash(data: &[u8], pos: usize, hash_log: u32) -> u32 {
        if pos + 4 > data.len() {
            return 0;
        }
        let h = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        // Simple multiply-shift hash.
        (h.wrapping_mul(2654435761) >> (32 - hash_log)) & ((1u32 << hash_log) - 1)
    }

    /// Insert position `pos` into the LDM hash table (sparse sampling).
    pub fn insert(&mut self, data: &[u8], pos: usize) {
        if pos % self.gap != 0 {
            return;
        }
        let h = Self::hash(data, pos, self.hash_log) as usize;
        let chain_idx = pos / self.gap;
        if chain_idx < self.chain.len() {
            self.chain[chain_idx] = self.head[h];
        }
        self.head[h] = pos as u32;
    }

    /// Find the longest LDM match at `pos` in `data`.
    ///
    /// Walks the hash chain up to `max_chain` entries. Returns the
    /// longest match with distance ≤ `max_distance`.
    pub fn find_match(
        &self,
        data: &[u8],
        pos: usize,
        max_distance: u32,
        max_chain: u32,
        min_match: u32,
    ) -> Option<LdmMatch> {
        if pos + 4 > data.len() {
            return None;
        }
        let h = Self::hash(data, pos, self.hash_log) as usize;
        let mut candidate = self.head[h];
        let mut best_len = 0u32;
        let mut best_dist = 0u32;
        let mut chain_count = 0u32;

        while candidate != u32::MAX && chain_count < max_chain {
            let cand = candidate as usize;
            if cand >= pos {
                // Candidate is at or after the current position (can
                // happen when the table is pre-populated over the full
                // input). Skip it but keep walking the chain to find
                // earlier candidates. Don't count this skip against
                // chain_count so we still search the full budget of
                // valid backward entries.
                let chain_idx = cand / self.gap;
                if chain_idx < self.chain.len() {
                    candidate = self.chain[chain_idx];
                } else {
                    break;
                }
                continue;
            }
            let dist = (pos - cand) as u32;
            if dist > max_distance {
                break;
            }

            // Extend match.
            let mut len = 0usize;
            while pos + len < data.len() && cand + len < pos && data[cand + len] == data[pos + len]
            {
                len += 1;
            }

            if len as u32 > best_len && len as u32 >= min_match {
                best_len = len as u32;
                best_dist = dist;
            }

            // Follow chain.
            let chain_idx = cand / self.gap;
            if chain_idx < self.chain.len() {
                candidate = self.chain[chain_idx];
            } else {
                break;
            }
            chain_count += 1;
        }

        if best_len >= min_match {
            Some(LdmMatch {
                distance: best_dist,
                length: best_len,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ldm_finds_long_distance_match() {
        let mut data = vec![0u8; 100_000];
        // Place a pattern at position 0 and position 50_000.
        for i in 0..256 {
            data[i] = (i % 251) as u8;
            data[50_000 + i] = (i % 251) as u8;
        }

        let mut ldm = LdmHashTable::new(20, 64);
        // Insert all positions.
        for pos in 0..data.len() {
            ldm.insert(&data, pos);
        }

        // At position 50_000, should find a match at distance 50_000.
        let m = ldm.find_match(&data, 50_000, u32::MAX, 32, 4);
        assert!(m.is_some(), "should find LDM match");
        let m = m.unwrap();
        assert!(
            m.length >= 200,
            "match length {} should be >= 200",
            m.length
        );
        assert_eq!(m.distance, 50_000, "distance should be 50000");
    }

    #[test]
    fn ldm_no_false_match_on_random() {
        let data: Vec<u8> = (0..10_000u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 16) as u8)
            .collect();

        let mut ldm = LdmHashTable::new(16, 64);
        for pos in 0..data.len() {
            ldm.insert(&data, pos);
        }

        // Random data should rarely produce matches ≥ 4 bytes.
        let mut matches = 0;
        for pos in 0..data.len() {
            if ldm.find_match(&data, pos, u32::MAX, 8, 4).is_some() {
                matches += 1;
            }
        }
        // Very few matches expected on pseudo-random data.
        assert!(
            matches < data.len() / 10,
            "too many false matches: {matches}"
        );
    }
}
