//! Predefined FSE decode tables — extracted verbatim from the zstd C
//! source (v1.5.7, `lib/decompress/zstd_decompress_block.c` lines
//! 364–460). These are pre-computed at compile time in the C source;
//! we hard-code them for exact interoperability.
//!
//! Each entry is `(next_state, nb_add_bits, nb_bits, base_val)`:
//! - `next_state`: baseline for FSE state transition
//! - `nb_add_bits`: extra bits to read for LL/ML/OF value
//! - `nb_bits`: FSE bits to read for next state
//! - `base_val`: base value for LL/ML/OF computation

#![forbid(unsafe_code)]
// Values are copied verbatim from the C source; preserving the unbroken
// digit form makes line-by-line verification against the C source easier.
#![allow(clippy::unreadable_literal)]

/// One entry in a predefined FSE decode table. Matches the C source's
/// `ZSTD_seqSymbol` layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PredefEntry {
    pub next_state: u16,
    pub nb_add_bits: u8,
    pub nb_bits: u8,
    pub base_val: u32,
}

/// Predefined Literal Length table (`accuracy_log` = 6, 64 entries).
/// Source: zstd v1.5.7 `LL_defaultDTable`.
pub const LL_PREDEF: [PredefEntry; 64] = [
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 4, base_val:    0 },
    PredefEntry { next_state: 16, nb_add_bits: 0, nb_bits: 4, base_val:    0 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:    1 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:    3 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:    4 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:    6 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:    7 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:    9 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:   10 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:   12 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:   14 },
    PredefEntry { next_state:  0, nb_add_bits: 1, nb_bits: 5, base_val:   16 },
    PredefEntry { next_state:  0, nb_add_bits: 1, nb_bits: 5, base_val:   20 },
    PredefEntry { next_state:  0, nb_add_bits: 1, nb_bits: 5, base_val:   22 },
    PredefEntry { next_state:  0, nb_add_bits: 2, nb_bits: 5, base_val:   28 },
    PredefEntry { next_state:  0, nb_add_bits: 3, nb_bits: 5, base_val:   32 },
    PredefEntry { next_state:  0, nb_add_bits: 4, nb_bits: 5, base_val:   48 },
    PredefEntry { next_state: 32, nb_add_bits: 6, nb_bits: 5, base_val:   64 },
    PredefEntry { next_state:  0, nb_add_bits: 7, nb_bits: 5, base_val:  128 },
    PredefEntry { next_state:  0, nb_add_bits: 8, nb_bits: 6, base_val:  256 },
    PredefEntry { next_state:  0, nb_add_bits: 10, nb_bits: 6, base_val: 1024 },
    PredefEntry { next_state:  0, nb_add_bits: 12, nb_bits: 6, base_val: 4096 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 4, base_val:    0 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 4, base_val:    1 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:    2 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:    4 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:    5 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:    7 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:    8 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:   10 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:   11 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:   13 },
    PredefEntry { next_state: 32, nb_add_bits: 1, nb_bits: 5, base_val:   16 },
    PredefEntry { next_state:  0, nb_add_bits: 1, nb_bits: 5, base_val:   18 },
    PredefEntry { next_state: 32, nb_add_bits: 1, nb_bits: 5, base_val:   22 },
    PredefEntry { next_state:  0, nb_add_bits: 2, nb_bits: 5, base_val:   24 },
    PredefEntry { next_state: 32, nb_add_bits: 3, nb_bits: 5, base_val:   32 },
    PredefEntry { next_state:  0, nb_add_bits: 3, nb_bits: 5, base_val:   40 },
    PredefEntry { next_state:  0, nb_add_bits: 6, nb_bits: 4, base_val:   64 },
    PredefEntry { next_state: 16, nb_add_bits: 6, nb_bits: 4, base_val:   64 },
    PredefEntry { next_state: 32, nb_add_bits: 7, nb_bits: 5, base_val:  128 },
    PredefEntry { next_state:  0, nb_add_bits: 9, nb_bits: 6, base_val:  512 },
    PredefEntry { next_state:  0, nb_add_bits: 11, nb_bits: 6, base_val: 2048 },
    PredefEntry { next_state: 48, nb_add_bits: 0, nb_bits: 4, base_val:    0 },
    PredefEntry { next_state: 16, nb_add_bits: 0, nb_bits: 4, base_val:    1 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:    2 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:    3 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:    5 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:    6 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:    8 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:    9 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:   11 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:   12 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:   15 },
    PredefEntry { next_state: 32, nb_add_bits: 1, nb_bits: 5, base_val:   18 },
    PredefEntry { next_state: 32, nb_add_bits: 1, nb_bits: 5, base_val:   20 },
    PredefEntry { next_state: 32, nb_add_bits: 2, nb_bits: 5, base_val:   24 },
    PredefEntry { next_state: 32, nb_add_bits: 2, nb_bits: 5, base_val:   28 },
    PredefEntry { next_state: 32, nb_add_bits: 3, nb_bits: 5, base_val:   40 },
    PredefEntry { next_state: 32, nb_add_bits: 4, nb_bits: 5, base_val:   48 },
    PredefEntry { next_state:  0, nb_add_bits: 16, nb_bits: 6, base_val: 65536 },
    PredefEntry { next_state:  0, nb_add_bits: 15, nb_bits: 6, base_val: 32768 },
    PredefEntry { next_state:  0, nb_add_bits: 14, nb_bits: 6, base_val: 16384 },
    PredefEntry { next_state:  0, nb_add_bits: 13, nb_bits: 6, base_val:  8192 },
];

/// Predefined Offset Code table (`accuracy_log` = 5, 32 entries).
/// Source: zstd v1.5.7 `OF_defaultDTable`.
pub const OF_PREDEF: [PredefEntry; 32] = [
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:        0 },
    PredefEntry { next_state:  0, nb_add_bits: 6, nb_bits: 4, base_val:       61 },
    PredefEntry { next_state:  0, nb_add_bits: 9, nb_bits: 5, base_val:      509 },
    PredefEntry { next_state:  0, nb_add_bits: 15, nb_bits: 5, base_val:    32765 },
    PredefEntry { next_state:  0, nb_add_bits: 21, nb_bits: 5, base_val:  2097149 },
    PredefEntry { next_state:  0, nb_add_bits: 3, nb_bits: 5, base_val:        5 },
    PredefEntry { next_state:  0, nb_add_bits: 7, nb_bits: 4, base_val:      125 },
    PredefEntry { next_state:  0, nb_add_bits: 12, nb_bits: 5, base_val:     4093 },
    PredefEntry { next_state:  0, nb_add_bits: 18, nb_bits: 5, base_val:   262141 },
    PredefEntry { next_state:  0, nb_add_bits: 23, nb_bits: 5, base_val:  8388605 },
    PredefEntry { next_state:  0, nb_add_bits: 5, nb_bits: 5, base_val:       29 },
    PredefEntry { next_state:  0, nb_add_bits: 8, nb_bits: 4, base_val:      253 },
    PredefEntry { next_state:  0, nb_add_bits: 14, nb_bits: 5, base_val:    16381 },
    PredefEntry { next_state:  0, nb_add_bits: 20, nb_bits: 5, base_val:  1048573 },
    PredefEntry { next_state:  0, nb_add_bits: 2, nb_bits: 5, base_val:        1 },
    PredefEntry { next_state: 16, nb_add_bits: 7, nb_bits: 4, base_val:      125 },
    PredefEntry { next_state:  0, nb_add_bits: 11, nb_bits: 5, base_val:     2045 },
    PredefEntry { next_state:  0, nb_add_bits: 17, nb_bits: 5, base_val:   131069 },
    PredefEntry { next_state:  0, nb_add_bits: 22, nb_bits: 5, base_val:  4194301 },
    PredefEntry { next_state:  0, nb_add_bits: 4, nb_bits: 5, base_val:       13 },
    PredefEntry { next_state: 16, nb_add_bits: 8, nb_bits: 4, base_val:      253 },
    PredefEntry { next_state:  0, nb_add_bits: 13, nb_bits: 5, base_val:     8189 },
    PredefEntry { next_state:  0, nb_add_bits: 19, nb_bits: 5, base_val:   524285 },
    PredefEntry { next_state:  0, nb_add_bits: 1, nb_bits: 5, base_val:        1 },
    PredefEntry { next_state: 16, nb_add_bits: 6, nb_bits: 4, base_val:       61 },
    PredefEntry { next_state:  0, nb_add_bits: 10, nb_bits: 5, base_val:     1021 },
    PredefEntry { next_state:  0, nb_add_bits: 16, nb_bits: 5, base_val:    65533 },
    PredefEntry { next_state:  0, nb_add_bits: 28, nb_bits: 5, base_val: 268435453 },
    PredefEntry { next_state:  0, nb_add_bits: 27, nb_bits: 5, base_val: 134217725 },
    PredefEntry { next_state:  0, nb_add_bits: 26, nb_bits: 5, base_val:  67108861 },
    PredefEntry { next_state:  0, nb_add_bits: 25, nb_bits: 5, base_val:  33554429 },
    PredefEntry { next_state:  0, nb_add_bits: 24, nb_bits: 5, base_val:  16777213 },
];

/// Predefined Match Length table (`accuracy_log` = 6, 64 entries).
/// Source: zstd v1.5.7 `ML_defaultDTable`.
pub const ML_PREDEF: [PredefEntry; 64] = [
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:      3 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 4, base_val:      4 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:      5 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:      6 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:      8 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:      9 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:     11 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     13 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     16 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     19 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     22 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     25 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     28 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     31 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     34 },
    PredefEntry { next_state:  0, nb_add_bits: 1, nb_bits: 6, base_val:     37 },
    PredefEntry { next_state:  0, nb_add_bits: 1, nb_bits: 6, base_val:     41 },
    PredefEntry { next_state:  0, nb_add_bits: 2, nb_bits: 6, base_val:     47 },
    PredefEntry { next_state:  0, nb_add_bits: 3, nb_bits: 6, base_val:     59 },
    PredefEntry { next_state:  0, nb_add_bits: 4, nb_bits: 6, base_val:     83 },
    PredefEntry { next_state:  0, nb_add_bits: 7, nb_bits: 6, base_val:    131 },
    PredefEntry { next_state:  0, nb_add_bits: 9, nb_bits: 6, base_val:    515 },
    PredefEntry { next_state: 16, nb_add_bits: 0, nb_bits: 4, base_val:      4 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 4, base_val:      5 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:      6 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:      7 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:      9 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 5, base_val:     10 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     12 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     15 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     18 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     21 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     24 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     27 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     30 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     33 },
    PredefEntry { next_state:  0, nb_add_bits: 1, nb_bits: 6, base_val:     35 },
    PredefEntry { next_state:  0, nb_add_bits: 1, nb_bits: 6, base_val:     39 },
    PredefEntry { next_state:  0, nb_add_bits: 2, nb_bits: 6, base_val:     43 },
    PredefEntry { next_state:  0, nb_add_bits: 3, nb_bits: 6, base_val:     51 },
    PredefEntry { next_state:  0, nb_add_bits: 4, nb_bits: 6, base_val:     67 },
    PredefEntry { next_state:  0, nb_add_bits: 5, nb_bits: 6, base_val:     99 },
    PredefEntry { next_state:  0, nb_add_bits: 8, nb_bits: 6, base_val:    259 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 4, base_val:      4 },
    PredefEntry { next_state: 48, nb_add_bits: 0, nb_bits: 4, base_val:      4 },
    PredefEntry { next_state: 16, nb_add_bits: 0, nb_bits: 4, base_val:      5 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:      7 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:      8 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:     10 },
    PredefEntry { next_state: 32, nb_add_bits: 0, nb_bits: 5, base_val:     11 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     14 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     17 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     20 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     23 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     26 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     29 },
    PredefEntry { next_state:  0, nb_add_bits: 0, nb_bits: 6, base_val:     32 },
    PredefEntry { next_state:  0, nb_add_bits: 16, nb_bits: 6, base_val:  65539 },
    PredefEntry { next_state:  0, nb_add_bits: 15, nb_bits: 6, base_val:  32771 },
    PredefEntry { next_state:  0, nb_add_bits: 14, nb_bits: 6, base_val:  16387 },
    PredefEntry { next_state:  0, nb_add_bits: 13, nb_bits: 6, base_val:   8195 },
    PredefEntry { next_state:  0, nb_add_bits: 12, nb_bits: 6, base_val:   4099 },
    PredefEntry { next_state:  0, nb_add_bits: 11, nb_bits: 6, base_val:   2051 },
    PredefEntry { next_state:  0, nb_add_bits: 10, nb_bits: 6, base_val:   1027 },
];

/// Accuracy log for each predefined table.
pub const LL_ACCURACY_LOG: u8 = 6;
pub const OF_ACCURACY_LOG: u8 = 5;
pub const ML_ACCURACY_LOG: u8 = 6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ll_table_has_64_entries() {
        assert_eq!(LL_PREDEF.len(), 1 << LL_ACCURACY_LOG);
    }

    #[test]
    fn of_table_has_32_entries() {
        assert_eq!(OF_PREDEF.len(), 1 << OF_ACCURACY_LOG);
    }

    #[test]
    fn ml_table_has_64_entries() {
        assert_eq!(ML_PREDEF.len(), 1 << ML_ACCURACY_LOG);
    }

    #[test]
    fn of_entry_0_is_repeat_offset() {
        // Entry 0 must be repeat offset (base_val ≤ 2).
        assert!(OF_PREDEF[0].base_val <= 2);
    }
}
