//! omnizip-lzma — Pure-Rust LZMA / LZMA2 / XZ.
//!
//! Rust port of omnizip's Ruby LZMA reference at
//! `omnizip/lib/omnizip/algorithms/lzma/` (7,558 LOC) and
//! `omnizip/lib/omnizip/algorithms/lzma2/` (906 LOC).
//!
//! See the workspace [`PLAN.md`](../../PLAN.md) for the Ruby → Rust module
//! map and the phased delivery plan.
//!
//! ## Status
//!
//! **Phase A decode-side: working for `.lzma` (LZMA-Alone).**
//!
//! The LZMA1 packet engine ([`decoder::Lzma1Decoder`]) drives every
//! container format; [`lzma_alone_decompress`] is the only top-level
//! entry wired so far. Differential tests against reference `xz -d`
//! pass on every fixture under `tests/fixtures/lzma/good-*.lzma`.
//!
//! Remaining Phase A work:
//! - XZ container decode (`decoder/xz.rs`): stream + block + CRC32/64.
//! - Lzip container decode (`decoder/lzip.rs`).
//! - LZMA2 multi-chunk decode (`decoder/lzma2.rs`).
//!
//! Encoder (with lazy parsing) and decoder.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
// Codec code performs extensive byte↔usize↔u32 conversions where the
// value ranges are guaranteed by upstream protocol checks. The pedantic
// cast lints fire on every such conversion without knowing the invariant;
// they're more useful at API boundaries than on every arithmetic site.
#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_lossless)]

pub mod bit_model;
pub mod codec;
pub mod constants;
pub mod coder;
pub mod crc32;
pub mod decoder;
pub mod dictionary;
pub mod encoder;
pub mod lzip;
pub mod lzma2;
pub mod r#match;
pub mod range_coder;
pub mod state;
pub mod xz_container;

use std::fmt;

pub use bit_model::{BitModel, BitModelArray};
pub use codec::LzmaCodec;
pub use coder::{DistanceDecoder, DistanceEncoder, LengthDecoder, LengthEncoder,
                 LiteralDecoder, LiteralEncoder};
pub use constants::{COMPRESSION_LEVEL_MAX, COMPRESSION_LEVEL_MIN};
pub use crc32::crc32;
pub use decoder::{lzma_alone_decompress, Lzma1Decoder};
pub use dictionary::Dictionary;
pub use encoder::{lzma_alone_compress, lzip_compress, xz_compress, Lzma1Encoder,
                   MatchFinder};
pub use lzip::lzip_decompress;
pub use lzma2::decode_lzma2_stream;
pub use r#match::Match;
pub use range_coder::{RangeDecoder, RangeEncoder};
pub use state::{LzmaState, LIT_STATES, MATCH_STATES, NUM_STATES, REP_STATES, SHORT_REP_STATES};
pub use xz_container::xz_decompress;

/// LZMA compression level 0–9, matching `xz` presets.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct LzmaLevel(u8);

impl LzmaLevel {
    /// Construct a level. Clamped to 0–9; values above 9 panic.
    ///
    /// # Panics
    ///
    /// Panics if `level > 9` ([`COMPRESSION_LEVEL_MAX`]).
    #[must_use]
    pub fn new(level: u8) -> Self {
        assert!(
            level <= COMPRESSION_LEVEL_MAX,
            "LZMA level must be 0..={COMPRESSION_LEVEL_MAX}, got {level}",
        );
        Self(level)
    }

    /// `xz -0` (fastest).
    #[must_use]
    pub const fn level_0() -> Self {
        Self(0)
    }

    /// `xz -6` (the `xz` default).
    #[must_use]
    pub const fn default() -> Self {
        Self(constants::COMPRESSION_LEVEL_DEFAULT)
    }

    /// `xz -9` (best ratio).
    #[must_use]
    pub const fn best() -> Self {
        Self(COMPRESSION_LEVEL_MAX)
    }

    /// Numeric level 0–9.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self.0
    }
}

impl fmt::Display for LzmaLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lzma-{}", self.0)
    }
}

/// Error type. Will grow as phases ship.
#[derive(Debug)]
pub enum LzmaError {
    /// Level not supported by this codec.
    LevelUnavailable(LzmaLevel),
    /// Malformed input.
    Corrupt { reason: String },
}

impl fmt::Display for LzmaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LevelUnavailable(level) => write!(f, "level {level} not supported by this codec"),
            Self::Corrupt { reason } => write!(f, "corrupt lzma stream: {reason}"),
        }
    }
}

impl std::error::Error for LzmaError {}

/// Compress `plaintext` with LZMA2 at the given level. Placeholder until
/// Phase C ships with match finder + lazy parsing.
///
/// # Errors
///
/// Returns [`LzmaError::LevelUnavailable`] until the encoder is ported.
pub fn lzma2_compress(_plaintext: &[u8], level: LzmaLevel) -> Result<Vec<u8>, LzmaError> {
    Err(LzmaError::LevelUnavailable(level))
}

/// Decompress an LZMA2 stream. Placeholder until Phase A (decoder port)
/// ships.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] until the decoder is wired in.
pub fn lzma2_decompress(_compressed: &[u8], _expected_len: u32) -> Result<Vec<u8>, LzmaError> {
    Err(LzmaError::Corrupt {
        reason: "decoder not available".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_clamps_and_displays() {
        assert_eq!(LzmaLevel::new(0).to_string(), "lzma-0");
        assert_eq!(LzmaLevel::default().to_string(), "lzma-5");
        assert_eq!(LzmaLevel::best().to_string(), "lzma-9");
    }

    #[test]
    #[should_panic(expected = "LZMA level must be 0..=9, got 12")]
    fn level_rejects_out_of_range() {
        let _ = LzmaLevel::new(12);
    }

    #[test]
    fn compress_reports_unavailable() {
        let err = lzma2_compress(b"abc", LzmaLevel::default()).unwrap_err();
        assert!(matches!(err, LzmaError::LevelUnavailable(_)));
    }
}
