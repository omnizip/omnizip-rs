//! omnizip-zstd — Pure-Rust Zstandard.
//!
//! Rust port of omnizip's Ruby ZSTD reference at
//! `omnizip/lib/omnizip/algorithms/zstandard/` (3,150 LOC).
//!
//! See the workspace [`PLAN.md`](../../PLAN.md) for the Ruby → Rust module
//! map and the phased delivery plan.
//!
//! ## Status
//!
//! **Phase A: foundation + RAW/RLE block decode working.** Constants,
//! frame header parser, FSE bitstream + table, block header, and the
//! top-level [`decoder::ZstdDecoder`] are ported. End-to-end decode
//! works for streams containing Raw / RLE blocks (small inputs that
//! `zstd` chooses not to compress). Compressed blocks need the
//! Huffman + literals + sequences stack, which lands next.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
// Codec code performs extensive byte↔usize↔u32 conversions where the
// value ranges are guaranteed by upstream protocol checks. The pedantic
// cast lints fire on every such conversion without knowing the invariant;
// they're more useful at API boundaries than on every arithmetic site.
#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_lossless)]

pub mod codec;
pub mod constants;
pub mod decoder;
pub mod frame;
pub mod fse;
pub mod huffman;
pub mod literals;
pub mod predef_tables;
pub mod sequences;

use std::fmt;

pub use codec::ZstdCodec;
pub use constants::{
    BLOCK_HEADER_SIZE, BLOCK_MAX_SIZE, BLOCK_TYPE_COMPRESSED, BLOCK_TYPE_RAW, BLOCK_TYPE_RLE,
    DEFAULT_LEVEL, FSE_MAX_ACCURACY_LOG, FSE_MIN_ACCURACY_LOG, MAGIC_BYTES, MAGIC_NUMBER,
    MAX_LEVEL, MIN_LEVEL, WINDOW_LOG_MAX, WINDOW_LOG_MIN,
};
pub use decoder::ZstdDecoder;
pub use frame::{detect_frame_kind, strip_magic, BlockHeader, FrameHeader};
pub use fse::{BitStream, ForwardBitStream, FseDecoder, FseState, Table};
pub use huffman::{HuffmanDecoder, HuffmanTable};
pub use literals::decode_literals_section;
pub use sequences::{decode_sequences_section, Sequence, SequenceExecutor,
                    SequencesSection};
/// ZSTD compression level. Mirrors the reference `zstd` encoder scale.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ZstdLevel {
    /// `zstd -1`.
    Fastest,
    /// `zstd -3`.
    Fast,
    /// `zstd -6` (the `zstd` default).
    Default,
    /// `zstd -12`.
    Better,
    /// `zstd -22` (best ratio, slowest encode).
    Best,
}

impl ZstdLevel {
    /// Numeric level matching the reference `zstd` encoder.
    #[must_use]
    pub fn as_reference_level(self) -> u8 {
        match self {
            Self::Fastest => 1,
            Self::Fast => 3,
            Self::Default => 6,
            Self::Better => 12,
            Self::Best => 22,
        }
    }
}

impl fmt::Display for ZstdLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "zstd-{}", self.as_reference_level())
    }
}

/// Error type. Will grow as phases ship.
#[derive(Debug)]
pub enum ZstdError {
    /// Level not yet wired in this build.
    LevelUnavailable(ZstdLevel),
    /// Malformed input.
    Corrupt { reason: String },
    /// Feature not yet implemented (e.g., compressed blocks before the
    /// Huffman + literals + sequences stack lands).
    Unsupported { reason: String },
}

impl fmt::Display for ZstdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LevelUnavailable(level) => write!(f, "level {level} not yet implemented"),
            Self::Corrupt { reason } => write!(f, "corrupt zstd frame: {reason}"),
            Self::Unsupported { reason } => write!(f, "unsupported: {reason}"),
        }
    }
}

impl std::error::Error for ZstdError {}

/// Compress `plaintext` at the given level. Placeholder until encoder
/// phases ship.
///
/// # Errors
///
/// Returns [`ZstdError::LevelUnavailable`] until the encoder is ported.
pub fn compress(_plaintext: &[u8], level: ZstdLevel) -> Result<Vec<u8>, ZstdError> {
    Err(ZstdError::LevelUnavailable(level))
}

/// Decompress a ZSTD frame.
///
/// Currently decodes Raw, RLE, and Compressed blocks (Compressed
/// requires the literals + sequences + executor stack). The
/// Huffman-FSE-compressed-weights path is not yet implemented (see
/// BUGREPORT.01).
///
/// # Errors
///
/// See [`ZstdDecoder::decode_stream`].
pub fn decompress(compressed: &[u8], _expected_len: u32) -> Result<Vec<u8>, ZstdError> {
    let mut decoder = ZstdDecoder::new();
    decoder.decode_stream(compressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_displays_reference_value() {
        assert_eq!(ZstdLevel::Fastest.to_string(), "zstd-1");
        assert_eq!(ZstdLevel::Default.to_string(), "zstd-6");
        assert_eq!(ZstdLevel::Best.to_string(), "zstd-22");
    }

    #[test]
    fn compress_reports_unavailable() {
        let err = compress(b"abc", ZstdLevel::Default).unwrap_err();
        assert!(matches!(err, ZstdError::LevelUnavailable(_)));
    }
}
