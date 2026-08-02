//! ZSTD compression parameters — ported from
//! `~/src/external/zstd/lib/compress/clevels.h`.
//!
//! Each compression level maps to a specific set of parameters that
//! control match finding, parsing strategy, and entropy coding. The
//! table is the exact C reference `ZSTD_defaultCParameters[0]` for
//! inputs > 256 KB.

#![forbid(unsafe_code)]

/// Parsing strategy (matches C `ZSTD_strategy` enum).
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Strategy {
    Fast,
    DoubleFast,
    Greedy,
    Lazy,
    Lazy2,
    Btlazy2,
    Btopt,
    Btultra,
    Btultra2,
}

/// Compression parameters for one ZSTD level. Matches the C
/// `ZSTD_compressionParameters` struct.
#[derive(Clone, Copy, Debug)]
pub struct CompressionParams {
    /// Window log (affects max match distance).
    pub window_log: u32,
    /// Chain log (hash chain depth).
    pub chain_log: u32,
    /// Hash log (hash table size = 1 << hash_log).
    pub hash_log: u32,
    /// Search log (number of search iterations).
    pub search_log: u32,
    /// Minimum match length.
    pub min_match: u32,
    /// Target length (step size for fast mode, search depth for lazy).
    pub target_length: u32,
    /// Parsing strategy.
    pub strategy: Strategy,
}

/// Get compression parameters for `level` (1–22). Uses the "default"
/// table from `clevels.h` (srcSize > 256 KB).
///
/// # Panics
///
/// Panics if `level` is 0 or > 22.
#[must_use]
pub fn get_params(level: u8) -> CompressionParams {
    assert!(level >= 1 && level <= 22, "ZSTD level must be 1..=22");
    // Table: [windowLog, chainLog, hashLog, searchLog, minMatch, targetLength, strategy]
    // Ported verbatim from ZSTD_defaultCParameters[0] in clevels.h.
    const TABLE: [(u32, u32, u32, u32, u32, u32, Strategy); 22] = [
        (19, 13, 14, 1, 7, 0, Strategy::Fast),       // L1
        (20, 15, 16, 1, 6, 0, Strategy::Fast),       // L2
        (21, 16, 17, 1, 5, 0, Strategy::DoubleFast), // L3
        (21, 18, 18, 1, 5, 0, Strategy::DoubleFast), // L4
        (21, 18, 19, 3, 5, 2, Strategy::Greedy),     // L5
        (21, 18, 19, 3, 5, 4, Strategy::Lazy),       // L6
        (21, 19, 20, 4, 5, 8, Strategy::Lazy),       // L7
        (21, 19, 20, 4, 5, 16, Strategy::Lazy2),     // L8
        (22, 20, 21, 4, 5, 16, Strategy::Lazy2),     // L9
        (22, 21, 22, 5, 5, 16, Strategy::Lazy2),     // L10
        (22, 21, 22, 6, 5, 16, Strategy::Lazy2),     // L11
        (22, 22, 23, 6, 5, 32, Strategy::Lazy2),     // L12
        (22, 22, 22, 4, 5, 32, Strategy::Btlazy2),   // L13
        (22, 22, 23, 5, 5, 32, Strategy::Btlazy2),   // L14
        (22, 23, 23, 6, 5, 32, Strategy::Btlazy2),   // L15
        (22, 22, 22, 5, 5, 48, Strategy::Btopt),     // L16
        (23, 23, 22, 5, 4, 64, Strategy::Btopt),     // L17
        (23, 23, 22, 6, 3, 64, Strategy::Btultra),   // L18
        (23, 24, 22, 7, 3, 256, Strategy::Btultra2), // L19
        (25, 25, 23, 7, 3, 256, Strategy::Btultra2), // L20
        (26, 26, 24, 7, 3, 512, Strategy::Btultra2), // L21
        (27, 27, 25, 9, 3, 999, Strategy::Btultra2), // L22
    ];
    let (wl, cl, hl, sl, mm, tl, st) = TABLE[(level - 1) as usize];
    CompressionParams {
        window_log: wl,
        chain_log: cl,
        hash_log: hl,
        search_log: sl,
        min_match: mm,
        target_length: tl,
        strategy: st,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_1_uses_fast_strategy() {
        let p = get_params(1);
        assert_eq!(p.strategy, Strategy::Fast);
        assert_eq!(p.hash_log, 14);
        assert_eq!(p.min_match, 7);
    }

    #[test]
    fn level_6_uses_lazy_strategy() {
        let p = get_params(6);
        assert_eq!(p.strategy, Strategy::Lazy);
        assert_eq!(p.hash_log, 19);
        assert_eq!(p.min_match, 5);
    }

    #[test]
    fn level_22_uses_btultra2() {
        let p = get_params(22);
        assert_eq!(p.strategy, Strategy::Btultra2);
        assert_eq!(p.hash_log, 25);
        assert_eq!(p.min_match, 3);
    }

    #[test]
    fn higher_levels_have_larger_hash_tables() {
        let l1 = get_params(1);
        let l6 = get_params(6);
        let l22 = get_params(22);
        assert!(l1.hash_log < l6.hash_log);
        assert!(l6.hash_log < l22.hash_log);
    }
}
