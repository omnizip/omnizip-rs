//! Unified error type for every omnizip-rs codec.
//!
//! ## Structure
//!
//! `OmnizipError` carries a [`CodecId`] for filtering / dispatch.
//! The per-codec structured sub-errors (TODO 259) live alongside —
//! callers wanting maximum detail can match on both layers:
//!
//! ```ignore
//! match codec.compress(data, level) {
//!     Ok(out) => out,
//!     Err(OmnizipError::EncodeFailed { codec, reason }) => {
//!         eprintln!("{codec} failed: {reason}");
//!         // ...
//!     }
//!     Err(OmnizipError::LevelOutOfRange { codec, level, .. }) => {
//!         // ...
//!     }
//!     Err(e) => return Err(e),
//! }
//! ```

use crate::codec::CodecId;

/// Error returned by every codec operation in the omnizip-rs workspace.
#[derive(Debug)]
pub enum OmnizipError {
    /// The requested compression level is outside the codec's supported range.
    LevelOutOfRange {
        codec: CodecId,
        level: u8,
        min: u8,
        max: u8,
    },
    /// The codec is decode-only in this build (e.g., LZMA before Phase B).
    Unsupported { codec: CodecId, reason: String },
    /// The encoder returned a lower-level error.
    EncodeFailed { codec: CodecId, reason: String },
    /// The decoder returned a lower-level error.
    DecodeFailed { codec: CodecId, reason: String },
    /// Decompression succeeded but the output length doesn't match the
    /// `expected_len` recorded in the drop record. The data is corrupt.
    LengthMismatch {
        codec: CodecId,
        expected: u32,
        actual: usize,
    },
    /// Input is structurally invalid before decode even begins (e.g.,
    /// `expected_len` exceeds `usize`, or a length field is impossibly
    /// large for the available input).
    Corrupt { codec: CodecId, reason: String },
}

impl std::fmt::Display for OmnizipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LevelOutOfRange {
                codec,
                level,
                min,
                max,
            } => write!(
                f,
                "codec {codec}: level {level} out of range [{min}..={max}]"
            ),
            Self::Unsupported { codec, reason } => {
                write!(f, "codec {codec}: unsupported — {reason}")
            }
            Self::EncodeFailed { codec, reason } => {
                write!(f, "codec {codec}: encode failed — {reason}")
            }
            Self::DecodeFailed { codec, reason } => {
                write!(f, "codec {codec}: decode failed — {reason}")
            }
            Self::LengthMismatch {
                codec,
                expected,
                actual,
            } => write!(
                f,
                "codec {codec}: length mismatch — expected {expected}, got {actual}"
            ),
            Self::Corrupt { codec, reason } => {
                write!(f, "codec {codec}: corrupt — {reason}")
            }
        }
    }
}

impl std::error::Error for OmnizipError {}

/// Helper constructors. These let codecs write
/// `OmnizipError::encode_failed(CodecId::BROTLI, "missing header")`
/// instead of the verbose struct-literal form.
impl OmnizipError {
    /// Construct an `EncodeFailed` with the given reason string.
    #[must_use]
    pub fn encode_failed(codec: CodecId, reason: impl Into<String>) -> Self {
        Self::EncodeFailed {
            codec,
            reason: reason.into(),
        }
    }

    /// Construct a `DecodeFailed` with the given reason string.
    #[must_use]
    pub fn decode_failed(codec: CodecId, reason: impl Into<String>) -> Self {
        Self::DecodeFailed {
            codec,
            reason: reason.into(),
        }
    }

    /// Construct an `Unsupported` error.
    #[must_use]
    pub fn unsupported(codec: CodecId, reason: impl Into<String>) -> Self {
        Self::Unsupported {
            codec,
            reason: reason.into(),
        }
    }

    /// Construct a `Corrupt` error.
    #[must_use]
    pub fn corrupt(codec: CodecId, reason: impl Into<String>) -> Self {
        Self::Corrupt {
            codec,
            reason: reason.into(),
        }
    }

    /// Construct a `LengthMismatch` error.
    #[must_use]
    pub fn length_mismatch(codec: CodecId, expected: u32, actual: usize) -> Self {
        Self::LengthMismatch {
            codec,
            expected,
            actual,
        }
    }

    /// Construct a `LevelOutOfRange` error.
    #[must_use]
    pub fn level_out_of_range(codec: CodecId, level: u8, min: u8, max: u8) -> Self {
        Self::LevelOutOfRange {
            codec,
            level,
            min,
            max,
        }
    }

    /// Returns the `CodecId` associated with this error, if any.
    /// All current variants carry a codec id, so this always returns `Some`.
    #[must_use]
    pub const fn codec_id(&self) -> CodecId {
        match self {
            Self::LevelOutOfRange { codec, .. }
            | Self::Unsupported { codec, .. }
            | Self::EncodeFailed { codec, .. }
            | Self::DecodeFailed { codec, .. }
            | Self::LengthMismatch { codec, .. }
            | Self::Corrupt { codec, .. } => *codec,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_build_correct_variants() {
        let e = OmnizipError::encode_failed(CodecId::BROTLI, "boom");
        assert!(matches!(
            e,
            OmnizipError::EncodeFailed {
                codec: CodecId::BROTLI,
                ..
            }
        ));
        assert_eq!(e.codec_id(), CodecId::BROTLI);
        // CodecId displays as 0x0004 for BROTLI; just check the prefix.
        let s = e.to_string();
        assert!(s.contains("0x"), "got: {s}");
        assert!(s.contains("encode failed"));
        assert!(s.contains("boom"));

        let e = OmnizipError::level_out_of_range(CodecId::LZMA, 100, 0, 9);
        assert!(matches!(
            e,
            OmnizipError::LevelOutOfRange { level: 100, .. }
        ));
    }

    #[test]
    fn all_variants_have_codec_id() {
        let cases = [
            OmnizipError::encode_failed(CodecId::BROTLI, "x"),
            OmnizipError::decode_failed(CodecId::BROTLI, "x"),
            OmnizipError::unsupported(CodecId::BROTLI, "x"),
            OmnizipError::corrupt(CodecId::BROTLI, "x"),
            OmnizipError::length_mismatch(CodecId::BROTLI, 0, 1),
            OmnizipError::level_out_of_range(CodecId::BROTLI, 0, 0, 1),
        ];
        for e in cases {
            assert_eq!(e.codec_id(), CodecId::BROTLI);
        }
    }
}
