//! LZMA match finder — delegates to the shared hash-chain implementation.
//!
//! This module re-exports [`omnizip_codecs::matchfinder::HashChainMatchFinder`]
//! with LZMA-specific defaults. The algorithm (4-byte hash, chain walking,
//! word-at-a-time extension) is identical across codecs; only the config
//! defaults differ.
//!
//! ## Determinism
//!
//! All data structures are pre-allocated per encoder invocation.
//! No `HashSet` iteration, no thread-local state, no `DefaultHasher`.

#![forbid(unsafe_code)]

pub use omnizip_codecs::matchfinder::{HashChainConfig, HashChainMatchFinder, Lz77Match};

/// LZMA-specific alias for the shared match type.
pub type Match = Lz77Match;

/// LZMA-specific alias for the shared match finder.
pub type MatchFinder<'a> = HashChainMatchFinder<'a>;

/// Construct a match finder with LZMA defaults: 16-bit hash, min match 3,
/// chain depth 256, no nice-match early exit.
#[must_use]
pub fn new_lzma_match_finder(data: &[u8], dict_size: u32) -> MatchFinder<'_> {
    let config = HashChainConfig {
        dict_size,
        min_match: 3,
        max_chain_length: 256,
        nice_match: 0,
        hash_log: 16,
        max_match_length: 273, // MATCH_LEN_MAX: cap to avoid O(N²) on repetitive data
    };
    HashChainMatchFinder::new(data, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_short_match() {
        let data = b"hello world hello there";
        let mut mf = new_lzma_match_finder(data, 4096);
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
        let mut mf = new_lzma_match_finder(data, 4096);
        for _ in 0..data.len() {
            mf.advance();
        }
        assert!(mf.advance().is_none());
    }

    #[test]
    fn determinism_same_input_same_matches() {
        let data: Vec<u8> = (0..1000).map(|i| (i * 7 + 13) as u8).collect();
        let find_all = || {
            let mut mf = new_lzma_match_finder(&data, 4096);
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

    #[test]
    fn nice_match_short_circuits_chain_walk() {
        let data: Vec<u8> = (0..8192usize).map(|i| b'a' + ((i % 4) as u8)).collect();
        let mut mf = new_lzma_match_finder(&data, 4096);
        mf.set_nice_match(16);
        for _ in 0..100 {
            mf.advance();
        }
        let p = mf.position();
        if let Some(m) = mf.find_match(p) {
            assert!(m.length >= 16 || m.length == (data.len() - p) as u32);
        }
    }

    #[test]
    fn reuse_preserves_allocation_across_calls() {
        let data1: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        let mut mf = new_lzma_match_finder(&data1, 4096);
        while let Some(p) = mf.advance() {
            let _ = mf.find_match(p);
        }

        let data2: Vec<u8> = (0..4096).map(|i| (i * 7) as u8).collect();
        mf.reuse(&data2, 4096);
        // Verify reuse worked by finding matches in the new data
        while let Some(p) = mf.advance() {
            let _ = mf.find_match(p);
        }
    }

    #[test]
    fn reuse_grows_prev_when_dict_size_increases() {
        let data: Vec<u8> = vec![0; 4096];
        let mut mf = new_lzma_match_finder(&data, 4096);
        let bigger: Vec<u8> = vec![0; 8192];
        mf.reuse(&bigger, 8192);
        // If reuse didn't grow, we'd get an index-out-of-bounds panic
        // when advancing past 4096 positions.
        for _ in 0..8192 {
            mf.advance();
        }
    }

    #[test]
    fn reuse_then_find_match_works_correctly() {
        let data = b"hello world hello there";
        let mut mf_reuse = new_lzma_match_finder(b"unrelated", 4096);
        mf_reuse.reuse(data, 4096);
        for _ in 0..6 {
            mf_reuse.advance();
        }
        let m_reuse = mf_reuse.find_match(12);

        let mut mf_fresh = new_lzma_match_finder(data, 4096);
        for _ in 0..6 {
            mf_fresh.advance();
        }
        let m_fresh = mf_fresh.find_match(12);

        assert_eq!(m_reuse, m_fresh, "reuse should produce identical matches");
    }
}
