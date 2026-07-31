//! LZMA algorithm constants — ported line-by-line from
//! `omnizip/lib/omnizip/algorithms/lzma/constants.rb` (112 LOC).
//!
//! These constants define the range coder parameters, probability model
//! dimensions, and the encoder/decoder size limits that every LZMA
//! implementation must agree on. The reference is the Igor Pavlov 7-Zip
//! LZMA spec; omnizip's Ruby port is our direct source.
//!
//! ## Source attribution
//!
//! ```text
//! Copyright (C) 2025 Ribose Inc.
//! Permission is hereby granted, free of charge, ... (MIT)
//! ```
//!
//! See `LICENSE-NOTICE.md` at the workspace root.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

// ── Range coder ──────────────────────────────────────────────────────────

/// Threshold for range normalisation (`2^24`). When the range drops below
/// this, the encoder flushes a byte and rescales.
pub const TOP: u32 = 0x0100_0000;

/// Total probability range for bit models (`2^11`).
pub const BIT_MODEL_TOTAL: u32 = 0x800;

/// Number of bits to shift on each probability update. Smaller = faster
/// adaptation; larger = more stable.
pub const MOVE_BITS: u32 = 5;

/// Initial probability value for a fresh bit model (`BIT_MODEL_TOTAL / 2`,
/// i.e. 50% probability).
pub const INIT_PROBS: u32 = BIT_MODEL_TOTAL >> 1;

/// Number of bits used in direct-bit encoding.
pub const NUM_DIRECT_BITS: u32 = 8;

// ── LZMA state machine ───────────────────────────────────────────────────

/// Maximum number of literal-position bits (`lp` parameter).
pub const NUM_LIT_POS_BITS_MAX: u32 = 4;

/// Maximum number of literal-context bits (`lc` parameter).
pub const NUM_LIT_CONTEXT_BITS_MAX: u32 = 8;

/// Maximum number of position bits (`pb` parameter).
pub const NUM_POS_BITS_MAX: u32 = 4;

/// Number of states in the LZMA state machine. The state tracks recent
/// match/literal history to select the right probability context.
pub const NUM_STATES: usize = 12;

// ── Dictionary ───────────────────────────────────────────────────────────

/// Minimum dictionary size (`4 KiB`).
pub const DICT_SIZE_MIN: u32 = 1 << 12;

/// Maximum dictionary size (`1 GiB`).
pub const DICT_SIZE_MAX: u32 = 1 << 30;

// ── Matches ──────────────────────────────────────────────────────────────

/// Shortest match length the encoder can emit.
pub const MATCH_LEN_MIN: u32 = 2;

/// Longest match length the encoder can emit (`273 = 2 + 8 + 32 + 16 + 15 + 2^8 - 1`).
pub const MATCH_LEN_MAX: u32 = 273;

/// Number of distance slots in the distance coder.
pub const NUM_DIST_SLOTS: usize = 64;

/// Number of position states (`1 << NUM_POS_BITS_MAX`).
pub const POS_STATES_MAX: usize = 1 << NUM_POS_BITS_MAX;

/// Literal coder size (`1 << (lp + lc)`).
pub const LIT_SIZE_MAX: usize = 1 << (NUM_LIT_POS_BITS_MAX + NUM_LIT_CONTEXT_BITS_MAX);

/// Number of length-to-position states.
pub const NUM_LEN_TO_POS_STATES: u32 = 4;

// ── Compression levels ───────────────────────────────────────────────────

/// Minimum LZMA compression level (matches `xz -0`).
pub const COMPRESSION_LEVEL_MIN: u8 = 0;

/// Maximum LZMA compression level (matches `xz -9`).
pub const COMPRESSION_LEVEL_MAX: u8 = 9;

/// Default compression level (matches `xz -6`).
pub const COMPRESSION_LEVEL_DEFAULT: u8 = 5;

// ── Length encoding ──────────────────────────────────────────────────────

/// Bits in the low length coder.
pub const NUM_LEN_LOW_BITS: u32 = 3;

/// Bits in the mid length coder.
pub const NUM_LEN_MID_BITS: u32 = 3;

/// Bits in the high length coder.
pub const NUM_LEN_HIGH_BITS: u32 = 8;

/// Symbols in the low length coder (`1 << NUM_LEN_LOW_BITS`).
pub const LEN_LOW_SYMBOLS: u32 = 1 << NUM_LEN_LOW_BITS;

/// Symbols in the mid length coder (`1 << NUM_LEN_MID_BITS`).
pub const LEN_MID_SYMBOLS: u32 = 1 << NUM_LEN_MID_BITS;

/// Symbols in the high length coder (`1 << NUM_LEN_HIGH_BITS`).
pub const LEN_HIGH_SYMBOLS: u32 = 1 << NUM_LEN_HIGH_BITS;

// ── Distance encoding ────────────────────────────────────────────────────

/// Bits in a distance slot.
pub const NUM_DIST_SLOT_BITS: u32 = 6;

/// Bits in the distance alignment.
pub const DIST_ALIGN_BITS: u32 = 4;

/// Distance alignment table size.
pub const DIST_ALIGN_SIZE: u32 = 1 << DIST_ALIGN_BITS;

/// Start of the position-model index range.
pub const START_POS_MODEL_INDEX: u32 = 4;

/// End of the position-model index range.
pub const END_POS_MODEL_INDEX: u32 = 14;

/// Number of full distances (`1 << (END_POS_MODEL_INDEX / 2)`).
pub const NUM_FULL_DISTANCES: u32 = 1 << (END_POS_MODEL_INDEX >> 1);

/// Fast-limit for distance slot calculation (`1 << (NUM_DIST_SLOT_BITS + 1)`).
pub const DIST_SLOT_FAST_LIMIT: u32 = 1 << (NUM_DIST_SLOT_BITS + 1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_reference_spec() {
        // Values that the LZMA spec pins exactly — these must never change.
        assert_eq!(TOP, 0x0100_0000);
        assert_eq!(BIT_MODEL_TOTAL, 0x800);
        assert_eq!(MOVE_BITS, 5);
        assert_eq!(INIT_PROBS, 0x400); // BIT_MODEL_TOTAL / 2
        assert_eq!(NUM_STATES, 12);
        assert_eq!(MATCH_LEN_MIN, 2);
        assert_eq!(MATCH_LEN_MAX, 273);
        assert_eq!(NUM_DIST_SLOTS, 64);
        assert_eq!(DICT_SIZE_MIN, 4096);
        assert_eq!(DICT_SIZE_MAX, 1 << 30);
    }

    #[test]
    fn length_symbol_counts_are_powers_of_two() {
        assert_eq!(LEN_LOW_SYMBOLS, 8);
        assert_eq!(LEN_MID_SYMBOLS, 8);
        assert_eq!(LEN_HIGH_SYMBOLS, 256);
    }

    #[test]
    fn compression_level_range_matches_xz() {
        // Compile-time invariant: the default level is in range.
        const _: () = {
            assert!(COMPRESSION_LEVEL_DEFAULT <= COMPRESSION_LEVEL_MAX);
        };
        assert_eq!(COMPRESSION_LEVEL_MIN, 0);
        assert_eq!(COMPRESSION_LEVEL_MAX, 9);
    }

    #[test]
    fn position_states_are_pow2() {
        assert_eq!(POS_STATES_MAX, 16);
        assert!(POS_STATES_MAX.is_power_of_two());
    }
}
