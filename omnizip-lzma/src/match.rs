//! LZ77 match candidate — distance + length pair.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/lzma/match.rb` (32 LOC,
//! MIT, Ribose Inc.). Used by the encoder's match finder. The decoder
//! consumes matches emitted by the bitstream rather than constructing
//! them, but [`Match`] still appears in shared types and in tests.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::constants::{DICT_SIZE_MAX, MATCH_LEN_MIN};

/// A length/distance pair produced by the LZ77 match finder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Match {
    distance: u32,
    length: u32,
}

impl Match {
    /// Construct a match. Distances and lengths are stored as given;
    /// validation happens explicitly via [`Self::is_valid_for_dict`].
    #[must_use]
    pub const fn new(distance: u32, length: u32) -> Self {
        Self { distance, length }
    }

    /// The match distance — bytes between the current position and the
    /// start of the matched window.
    #[must_use]
    pub const fn distance(self) -> u32 {
        self.distance
    }

    /// The match length — bytes copied from the referenced window.
    #[must_use]
    pub const fn length(self) -> u32 {
        self.length
    }

    /// A match is valid iff its distance fits within `dict_size` and its
    /// length meets the LZMA minimum (`MATCH_LEN_MIN = 2`). Used by the
    /// encoder when filtering match-finder output.
    #[must_use]
    pub const fn is_valid_for_dict(self, dict_size: u32) -> bool {
        self.distance <= dict_size && self.length >= MATCH_LEN_MIN
    }

    /// A match is valid iff its distance is within the global maximum
    /// dictionary size and its length meets the minimum.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.is_valid_for_dict(DICT_SIZE_MAX)
    }
}

impl std::fmt::Display for Match {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Match(dist={}, len={})", self.distance, self.length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_and_reads_back() {
        let m = Match::new(123, 7);
        assert_eq!(m.distance(), 123);
        assert_eq!(m.length(), 7);
    }

    #[test]
    fn display_matches_ruby_format() {
        let m = Match::new(123, 7);
        assert_eq!(m.to_string(), "Match(dist=123, len=7)");
    }

    #[test]
    fn validity_requires_minimum_length() {
        // length 1 is below the LZMA minimum (2); reject.
        assert!(!Match::new(10, 1).is_valid_for_dict(4096));
        // length 2 is exactly the minimum; accept.
        assert!(Match::new(10, 2).is_valid_for_dict(4096));
    }

    #[test]
    fn validity_uses_dict_size() {
        let m = Match::new(8192, 5);
        assert!(m.is_valid_for_dict(8192));
        assert!(!m.is_valid_for_dict(4096));
    }

    #[test]
    fn equality_and_copy() {
        let a = Match::new(1, 2);
        let b = a;
        assert_eq!(a, b);
    }
}
