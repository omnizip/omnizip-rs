//! `LzmaCodec` — adapts the LZMA-Alone decoder to the
//! `omnizip_codecs::Codec` trait so it can be registered and dispatched
//! through the workspace codec registry.
//!
//! Phase A scope: `.lzma` (LZMA-Alone) decode only. Phase B will swap in
//! the LZMA2 / XZ-container encoder; Phase C will add the optimal parser.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

use crate::{lzma_alone_decompress, LzmaError};

/// Codec entry for the LZMA-Alone format (`.lzma` legacy container).
///
/// Decode is the only operation wired in Phase A. Compress returns
/// `LevelUnavailable` until the Phase B encoder lands.
pub struct LzmaCodec;

impl LzmaCodec {
    /// Construct a new codec instance. Stateless — the registry holds
    /// one shared instance.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for LzmaCodec {
    fn default() -> Self {
        Self::new()
    }
}

fn map_error(codec: CodecId, e: LzmaError) -> OmnizipError {
    OmnizipError::DecodeFailed {
        codec,
        reason: e.to_string(),
    }
}

impl Codec for LzmaCodec {
    fn id(&self) -> CodecId {
        CodecId::LZMA
    }

    fn name(&self) -> &'static str {
        "lzma-alone"
    }

    fn compress(
        &self,
        _plaintext: &[u8],
        level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        Err(OmnizipError::Unsupported {
            codec: CodecId::LZMA,
            reason: format!(
                "encode at level {level} not yet ported (LZMA Phase B — see TODO.omnizip-rs/11)"
            ),
        })
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let decoded = lzma_alone_decompress(compressed).map_err(|e| map_error(CodecId::LZMA, e))?;
        let expected = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::LZMA,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;
        if decoded.len() != expected {
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::LZMA,
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
    fn codec_id_is_lzma() {
        assert_eq!(LzmaCodec::new().id(), CodecId::LZMA);
    }

    #[test]
    fn name_is_stable() {
        assert_eq!(LzmaCodec::new().name(), "lzma-alone");
    }

    #[test]
    fn compress_returns_unsupported_in_phase_a() {
        let err = LzmaCodec::new()
            .compress(b"abc", CompressionLevel::default())
            .unwrap_err();
        assert!(matches!(err, OmnizipError::Unsupported { .. }));
    }
}
