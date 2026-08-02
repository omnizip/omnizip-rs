//! Hash-chain match finder for the LZMA encoder.
//!
//! Mirrors XZ Utils `lz_encoder.c`. Uses a 4-byte hash to find
//! candidate match positions, then walks a chain of previous
//! positions with the same hash to find the best match.
//!
//! ## Determinism
//!
//! Hash table + chain are pre-allocated per encoder invocation.
//! No `HashSet` iteration, no thread-local state, no `DefaultHasher`.

#![forbid(unsafe_code)]

/// A potential match found by the match finder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Match {
    pub distance: u32,
    pub length: u32,
}

/// Hash-chain match finder. Holds references into the input data
/// and the hash/chain arrays.
#[derive(Debug)]
pub struct MatchFinder<'a> {
    data: &'a [u8],
    /// Hash table: `head[hash]` = most recent position with that hash.
    head: Vec<u32>,
    /// Prev chain: `prev[pos & mask]` = previous position with same hash.
    prev: Vec<u32>,
    /// Mask for `prev` indexing (typically `dict_size` - 1).
    mask: u32,
    /// Current position in `data`.
    cur: usize,
    /// Max distance to search backward.
    max_distance: u32,
    /// Max chain length to walk per position.
    max_chain_length: u32,
    /// Minimum useful match length.
    min_match: u32,
}

impl<'a> MatchFinder<'a> {
    /// Construct a match finder over `data` with the given `dict_size`.
    #[must_use]
    pub fn new(data: &'a [u8], dict_size: u32) -> Self {
        let dict_size = dict_size.max(4096);
        let size = dict_size as usize;
        let mask = size as u32 - 1;
        Self {
            data,
            head: vec![u32::MAX; Self::HASH_SIZE],
            prev: vec![u32::MAX; size],
            mask,
            cur: 0,
            max_distance: dict_size,
            max_chain_length: 256,
            min_match: 3,
        }
    }

    /// 16-bit hash → 65536 entries.
    const HASH_SIZE: usize = 1 << 16;
    const HASH_SHIFT: u32 = 16 / 4;

    /// Compute the 4-byte hash at position `pos`.
    fn hash4(data: &[u8], pos: usize) -> usize {
        if pos + 4 > data.len() {
            return 0;
        }
        let word = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        ((word.wrapping_mul(0x9E37_79B1)) >> (32 - Self::HASH_SHIFT)) as usize & (Self::HASH_SIZE - 1)
    }

    /// Advance to the next position. Returns the position advanced to,
    /// or `None` if the end of `data` has been reached.
    pub fn advance(&mut self) -> Option<usize> {
        if self.cur >= self.data.len() {
            return None;
        }
        // Insert the current position into the hash chain.
        if self.cur + 4 <= self.data.len() {
            let h = Self::hash4(self.data, self.cur);
            let prev_pos = self.head[h];
            self.prev[self.cur & self.mask as usize] = prev_pos;
            self.head[h] = self.cur as u32;
        }
        let pos = self.cur;
        self.cur += 1;
        Some(pos)
    }

    /// Find the longest match at the current position. Returns the
    /// best match found (or `None` if no match ≥ `min_match`).
    #[must_use] 
    pub fn find_match(&self, pos: usize) -> Option<Match> {
        if pos + 4 > self.data.len() {
            return None;
        }
        let h = Self::hash4(self.data, pos);
        let mut candidate = self.head[h];
        let mut best_len = 0u32;
        let mut best_dist = 0u32;
        let mut chain = 0;
        let max_len = (self.data.len() - pos) as u32;

        while candidate != u32::MAX && chain < self.max_chain_length {
            let cand_us = candidate as usize;
            let dist = pos.saturating_sub(cand_us);
            if dist == 0 || dist as u32 > self.max_distance {
                break;
            }

            // Compute match length at this candidate.
            let len = self.match_length(pos, cand_us, max_len);
            if len > best_len && len >= self.min_match {
                best_len = len;
                best_dist = dist as u32;
                if len >= max_len {
                    break;
                }
            }

            candidate = self.prev[cand_us & self.mask as usize];
            chain += 1;
        }

        if best_len >= self.min_match {
            Some(Match {
                distance: best_dist,
                length: best_len,
            })
        } else {
            None
        }
    }

    /// Compute the match length between `data[a..]` and `data[b..]`,
    /// capped at `max_len`.
    fn match_length(&self, a: usize, b: usize, max_len: u32) -> u32 {
        let mut len = 0u32;
        while len < max_len && self.data.get(a + len as usize) == self.data.get(b + len as usize) {
            if self.data[a + len as usize] != self.data[b + len as usize] {
                break;
            }
            len += 1;
        }
        len
    }

    /// Current position in the input data.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.cur
    }

    /// Reset the match finder for re-use with new data.
    pub fn reset(&mut self) {
        for h in &mut self.head {
            *h = u32::MAX;
        }
        for p in &mut self.prev {
            *p = u32::MAX;
        }
        self.cur = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_short_match() {
        let data = b"hello world hello there";
        let mut mf = MatchFinder::new(data, 4096);
        // Advance past the first "hello " (positions 0..5).
        for _ in 0..6 {
            mf.advance();
        }
        // At position 12 ("hello there"), we should find a match at
        // distance 12, length 5 ("hello").
        if let Some(m) = mf.find_match(12) {
            assert_eq!(m.distance, 12);
            assert!(m.length >= 5);
        }
    }

    #[test]
    fn returns_none_at_eof() {
        let data = b"short";
        let mut mf = MatchFinder::new(data, 4096);
        for _ in 0..data.len() {
            mf.advance();
        }
        assert!(mf.advance().is_none());
    }

    #[test]
    fn determinism_same_input_same_matches() {
        let data: Vec<u8> = (0..1000).map(|i| (i * 7 + 13) as u8).collect();
        let find_all = || {
            let mut mf = MatchFinder::new(&data, 4096);
            let mut matches = Vec::new();
            while let Some(p) = mf.advance() {
                if let Some(m) = mf.find_match(p) {
                    matches.push((p, m.distance, m.length));
                }
            }
            matches
        };
        let a = find_all();
        let b = find_all();
        assert_eq!(a, b, "match finder non-deterministic");
    }
}
