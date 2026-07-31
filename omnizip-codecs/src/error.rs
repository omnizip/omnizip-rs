//! Unified error type for every omnizip-rs codec.

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
        }
    }
}

impl std::error::Error for OmnizipError {}
