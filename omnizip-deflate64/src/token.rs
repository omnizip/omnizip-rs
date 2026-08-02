//! LZ77 token type shared by encoder and decoder.

/// A single LZ77 token: either a raw literal byte or a (length, distance)
/// back-reference into the sliding window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    /// A literal byte.
    Literal {
        /// The byte value.
        value: u8,
    },
    /// A back-reference match.
    Match {
        /// Match length (3..=258).
        length: usize,
        /// Distance back into the window (1..=65536 for Deflate64).
        distance: usize,
    },
}
