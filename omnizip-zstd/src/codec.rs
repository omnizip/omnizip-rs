//! `ZstdCodec` — adapts the ZSTD decoder to the `omnizip_codecs::Codec`
//! trait so it can be registered and dispatched through the workspace
//! codec registry.
//!
//! Phase A: decode side handles Raw + RLE blocks (small inputs that
//! `zstd` chooses not to compress). Encode is Phase B.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

use crate::ZstdDecoder;

/// Codec entry for the Zstandard format.
pub struct ZstdCodec;

impl ZstdCodec {
    /// Construct a new codec instance. Stateless.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for ZstdCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl Codec for ZstdCodec {
    fn id(&self) -> CodecId {
        CodecId::ZSTD
    }

    fn name(&self) -> &'static str {
        "zstd"
    }

    fn compress(
        &self,
        _plaintext: &[u8],
        level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        Err(OmnizipError::Unsupported {
            codec: CodecId::ZSTD,
            reason: format!(
                "encode at level {} not yet ported (ZSTD Phase B — see TODO.omnizip-rs/14)",
                level
            ),
        })
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let mut decoder = ZstdDecoder::new();
        let decoded = decoder.decode_stream(compressed).map_err(|e| match e {
            crate::ZstdError::Unsupported { reason } => OmnizipError::Unsupported {
                codec: CodecId::ZSTD,
                reason,
            },
            other => OmnizipError::DecodeFailed {
                codec: CodecId::ZSTD,
                reason: other.to_string(),
            },
        })?;
        let expected = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::ZSTD,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;
        if decoded.len() != expected {
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::ZSTD,
                expected: expected_len,
                actual: decoded.len(),
            });
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_id_is_zstd() {
        assert_eq!(ZstdCodec::new().id(), CodecId::ZSTD);
    }

    #[test]
    fn name_is_stable() {
        assert_eq!(ZstdCodec::new().name(), "zstd");
    }

    #[test]
    fn compress_returns_unsupported_in_phase_a() {
        let err = ZstdCodec::new()
            .compress(b"abc", CompressionLevel::default())
            .unwrap_err();
        assert!(matches!(err, OmnizipError::Unsupported { .. }));
    }
}
