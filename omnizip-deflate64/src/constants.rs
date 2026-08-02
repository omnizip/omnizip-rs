//! Constants for Deflate64 (Enhanced Deflate) — ZIP method 9.
//!
//! Direct port of `omnizip/lib/omnizip/algorithms/deflate64/constants.rb`.
//! The defining difference from RFC 1951 DEFLATE is the 64 KB sliding
//! window ([`DICTIONARY_SIZE`]) vs the standard 32 KB.

#![allow(clippy::cast_possible_truncation)]

/// Dictionary / sliding-window size — 64 KB (vs 32 KB in standard DEFLATE).
pub const DICTIONARY_SIZE: usize = 65_536;

/// Maximum match length emitted by the LZ77 stage.
pub const MAX_MATCH_LENGTH: usize = 258;

/// Minimum match length — shorter matches are emitted as literals.
pub const MIN_MATCH_LENGTH: usize = 3;

/// Maximum match distance (= window size - 1).
pub const MAX_DISTANCE: usize = DICTIONARY_SIZE - 1;

/// Number of literal/length codes (0..285).
pub const LITERAL_CODES: usize = 286;

/// Number of distance codes (0..29). Deflate64 extends code 29 to cover
/// the full 32 KB..=64 KB distance range.
pub const DISTANCE_CODES: usize = 30;

/// End-of-block symbol — terminates the literal/length stream.
pub const END_OF_BLOCK: u16 = 256;

/// Hash table size for the LZ77 match finder (power of two for cheap mask).
pub const HASH_SIZE: usize = 65_536;

/// Shift used when rolling a 3-byte hash.
pub const HASH_SHIFT: u32 = 5;

/// Maximum candidate chain length examined per position.
pub const MAX_CHAIN_LENGTH: usize = 4096;

/// Length at which a match is considered "good enough" to stop searching.
pub const GOOD_MATCH: usize = 32;

/// Length at which a match is accepted immediately (`nice` match).
pub const NICE_MATCH: usize = 258;
