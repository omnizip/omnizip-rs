//! `LzmaCodec` — adapts the LZMA-Alone encoder + decoder to the
//! `omnizip_codecs::Codec` trait.

#![forbid(unsafe_code)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

use crate::encoder::xz_compress;
use crate::xz_container::xz_decompress;
use crate::LzmaError;

/// Codec entry for the LZMA family (XZ container with LZMA2 inside).
pub struct LzmaCodec;

impl LzmaCodec {
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

fn map_decode_error(e: LzmaError) -> OmnizipError {
    OmnizipError::DecodeFailed {
        codec: CodecId::LZMA,
        reason: e.to_string(),
    }
}

fn map_encode_error(e: LzmaError) -> OmnizipError {
    OmnizipError::EncodeFailed {
        codec: CodecId::LZMA,
        reason: e.to_string(),
    }
}

impl Codec for LzmaCodec {
    fn id(&self) -> CodecId {
        CodecId::LZMA
    }

    fn name(&self) -> &'static str {
        "lzma"
    }

    fn compress(
        &self,
        plaintext: &[u8],
        _level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        xz_compress(plaintext).map_err(map_encode_error)
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let decoded = xz_decompress(compressed).map_err(map_decode_error)?;
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
    fn round_trip_via_codec() {
        let codec = LzmaCodec::new();
        let input = b"hello codec world";
        let compressed = codec
            .compress(input, CompressionLevel::default())
            .expect("encode");
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .expect("decode");
        assert_eq!(decompressed, input);
    }
}
