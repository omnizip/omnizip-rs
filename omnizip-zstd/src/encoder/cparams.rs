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
    /// Hash log (hash table size = 1 << `hash_log`).
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

/// Get compression parameters for `level` (1–22) and an input of
/// `src_size` bytes — the size-tiered tables from `clevels.h`
/// (`tableID = (r <= 256K) + (r <= 128K) + (r <= 16K)`): small inputs
/// use smaller windows, smaller hashes, LOWER minMatch, and sometimes
/// a different strategy than the default table. The previous
/// single-table port used the >256 KB rows for every input, so a
/// 7-byte-hash L1 could never see the 5-6 byte matches the reference
/// finds on small files (the fonts 1.059x cell).
///
/// # Panics
///
/// Panics if `level` is 0 or > 22.
#[must_use]
pub fn get_params_for(level: u8, src_size: usize) -> CompressionParams {
    assert!((1..=22).contains(&level), "ZSTD level must be 1..=22");
    // Table: [windowLog, chainLog, hashLog, searchLog, minMatch, targetLength, strategy]
    // Ported verbatim from all four ZSTD_defaultCParameters tables in
    // clevels.h (row 0 = the negative-level base, kept for fidelity).
    const TIER0_GT_256K: [(u32, u32, u32, u32, u32, u32, Strategy); 23] = [
        (19, 12, 13, 1, 6, 1, Strategy::Fast),       // base
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
    const TIER1_LE_256K: [(u32, u32, u32, u32, u32, u32, Strategy); 23] = [
        (18, 12, 13, 1, 5, 1, Strategy::Fast),        // base
        (18, 13, 14, 1, 6, 0, Strategy::Fast),        // L1
        (18, 14, 14, 1, 5, 0, Strategy::DoubleFast),  // L2
        (18, 16, 16, 1, 4, 0, Strategy::DoubleFast),  // L3
        (18, 16, 17, 3, 5, 2, Strategy::Greedy),      // L4
        (18, 17, 18, 5, 5, 2, Strategy::Greedy),      // L5
        (18, 18, 19, 3, 5, 4, Strategy::Lazy),        // L6
        (18, 18, 19, 4, 4, 4, Strategy::Lazy),        // L7
        (18, 18, 19, 4, 4, 8, Strategy::Lazy2),       // L8
        (18, 18, 19, 5, 4, 8, Strategy::Lazy2),       // L9
        (18, 18, 19, 6, 4, 8, Strategy::Lazy2),       // L10
        (18, 18, 19, 5, 4, 12, Strategy::Btlazy2),    // L11
        (18, 19, 19, 7, 4, 12, Strategy::Btlazy2),    // L12
        (18, 18, 19, 4, 4, 16, Strategy::Btopt),      // L13
        (18, 18, 19, 4, 3, 32, Strategy::Btopt),      // L14
        (18, 18, 19, 6, 3, 128, Strategy::Btopt),     // L15
        (18, 19, 19, 6, 3, 128, Strategy::Btultra),   // L16
        (18, 19, 19, 8, 3, 256, Strategy::Btultra),   // L17
        (18, 19, 19, 6, 3, 128, Strategy::Btultra2),  // L18
        (18, 19, 19, 8, 3, 256, Strategy::Btultra2),  // L19
        (18, 19, 19, 10, 3, 512, Strategy::Btultra2), // L20
        (18, 19, 19, 12, 3, 512, Strategy::Btultra2), // L21
        (18, 19, 19, 13, 3, 999, Strategy::Btultra2), // L22
    ];
    const TIER2_LE_128K: [(u32, u32, u32, u32, u32, u32, Strategy); 23] = [
        (17, 12, 12, 1, 5, 1, Strategy::Fast),        // base
        (17, 12, 13, 1, 6, 0, Strategy::Fast),        // L1
        (17, 13, 15, 1, 5, 0, Strategy::Fast),        // L2
        (17, 15, 16, 2, 5, 0, Strategy::DoubleFast),  // L3
        (17, 17, 17, 2, 4, 0, Strategy::DoubleFast),  // L4
        (17, 16, 17, 3, 4, 2, Strategy::Greedy),      // L5
        (17, 16, 17, 3, 4, 4, Strategy::Lazy),        // L6
        (17, 16, 17, 3, 4, 8, Strategy::Lazy2),       // L7
        (17, 16, 17, 4, 4, 8, Strategy::Lazy2),       // L8
        (17, 16, 17, 5, 4, 8, Strategy::Lazy2),       // L9
        (17, 16, 17, 6, 4, 8, Strategy::Lazy2),       // L10
        (17, 17, 17, 5, 4, 8, Strategy::Btlazy2),     // L11
        (17, 18, 17, 7, 4, 12, Strategy::Btlazy2),    // L12
        (17, 18, 17, 3, 4, 12, Strategy::Btopt),      // L13
        (17, 18, 17, 4, 3, 32, Strategy::Btopt),      // L14
        (17, 18, 17, 6, 3, 256, Strategy::Btopt),     // L15
        (17, 18, 17, 6, 3, 128, Strategy::Btultra),   // L16
        (17, 18, 17, 8, 3, 256, Strategy::Btultra),   // L17
        (17, 18, 17, 10, 3, 512, Strategy::Btultra),  // L18
        (17, 18, 17, 5, 3, 256, Strategy::Btultra2),  // L19
        (17, 18, 17, 7, 3, 512, Strategy::Btultra2),  // L20
        (17, 18, 17, 9, 3, 512, Strategy::Btultra2),  // L21
        (17, 18, 17, 11, 3, 999, Strategy::Btultra2), // L22
    ];
    const TIER3_LE_16K: [(u32, u32, u32, u32, u32, u32, Strategy); 23] = [
        (14, 12, 13, 1, 5, 1, Strategy::Fast),        // base
        (14, 14, 15, 1, 5, 0, Strategy::Fast),        // L1
        (14, 14, 15, 1, 4, 0, Strategy::Fast),        // L2
        (14, 14, 15, 2, 4, 0, Strategy::DoubleFast),  // L3
        (14, 14, 14, 4, 4, 2, Strategy::Greedy),      // L4
        (14, 14, 14, 3, 4, 4, Strategy::Lazy),        // L5
        (14, 14, 14, 4, 4, 8, Strategy::Lazy2),       // L6
        (14, 14, 14, 6, 4, 8, Strategy::Lazy2),       // L7
        (14, 14, 14, 8, 4, 8, Strategy::Lazy2),       // L8
        (14, 15, 14, 5, 4, 8, Strategy::Btlazy2),     // L9
        (14, 15, 14, 9, 4, 8, Strategy::Btlazy2),     // L10
        (14, 15, 14, 3, 4, 12, Strategy::Btopt),      // L11
        (14, 15, 14, 4, 3, 24, Strategy::Btopt),      // L12
        (14, 15, 14, 5, 3, 32, Strategy::Btultra),    // L13
        (14, 15, 15, 6, 3, 64, Strategy::Btultra),    // L14
        (14, 15, 15, 7, 3, 256, Strategy::Btultra),   // L15
        (14, 15, 15, 5, 3, 48, Strategy::Btultra2),   // L16
        (14, 15, 15, 6, 3, 128, Strategy::Btultra2),  // L17
        (14, 15, 15, 7, 3, 256, Strategy::Btultra2),  // L18
        (14, 15, 15, 8, 3, 256, Strategy::Btultra2),  // L19
        (14, 15, 15, 8, 3, 512, Strategy::Btultra2),  // L20
        (14, 15, 15, 9, 3, 512, Strategy::Btultra2),  // L21
        (14, 15, 15, 10, 3, 999, Strategy::Btultra2), // L22
    ];
    let table_id = usize::from(src_size <= 256 * 1024)
        + usize::from(src_size <= 128 * 1024)
        + usize::from(src_size <= 16 * 1024);
    let tier = match table_id {
        0 => &TIER0_GT_256K,
        1 => &TIER1_LE_256K,
        2 => &TIER2_LE_128K,
        _ => &TIER3_LE_16K,
    };
    let (wl, cl, hl, sl, mm, tl, st) = tier[level as usize]; // row 0 is the base; L1 is row 1
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

/// Size-untiered variant (the >256 KB table) for callers without a
/// size hint.
#[must_use]
pub fn get_params(level: u8) -> CompressionParams {
    get_params_for(level, usize::MAX)
}

#[cfg(test)]
mod tests {
    #[test]
    fn size_tiers_match_clevels() {
        // >256 KB keeps the default table.
        let big = get_params_for(1, 257 * 1024);
        assert_eq!(big.min_match, 7);
        assert_eq!(big.strategy, Strategy::Fast);
        // <=256 KB: mml 6 at L1, dfast at L2 (clevels.h row 1).
        let t1 = get_params_for(1, 256 * 1024);
        assert_eq!(t1.min_match, 6);
        assert_eq!(t1.strategy, Strategy::Fast);
        let t1_l2 = get_params_for(2, 160 * 1024);
        assert_eq!(t1_l2.min_match, 5);
        assert_eq!(t1_l2.strategy, Strategy::DoubleFast);
        // <=128 KB and <=16 KB tiers selected at their bounds.
        let t2 = get_params_for(1, 128 * 1024);
        assert_eq!(t2.window_log, 17);
        let t3 = get_params_for(1, 16 * 1024);
        assert_eq!(t3.window_log, 14);
        assert_eq!(t3.min_match, 5);
    }

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
