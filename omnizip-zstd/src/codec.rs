//! `ZstdCodec` — adapts the ZSTD encoder + decoder to the
//! `omnizip_codecs::Codec` trait.

#![forbid(unsafe_code)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

use crate::{compress, decompress, ZstdDecoder, ZstdError, ZstdLevel};

/// Codec entry for the Zstandard format.
pub struct ZstdCodec;

impl ZstdCodec {
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

    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        let zstd_level = match level.as_u8() {
            0..=2 => ZstdLevel::Fastest,
            3..=9 => ZstdLevel::Fast,
            10..=16 => ZstdLevel::Default,
            17..=22 => ZstdLevel::Better,
            _ => ZstdLevel::Best,
        };
        compress(plaintext, zstd_level).map_err(|e| OmnizipError::EncodeFailed {
            codec: CodecId::ZSTD,
            reason: e.to_string(),
        })
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let _ = ZstdDecoder::new(); // ensure constructor is referenced
        let out = decompress(compressed, expected_len).map_err(|e| match e {
            ZstdError::Unsupported { reason } => OmnizipError::Unsupported {
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
        if out.len() != expected {
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::ZSTD,
                expected: expected_len,
                actual: out.len(),
            });
        }
        Ok(out)
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
    fn round_trip_via_codec() {
        let codec = ZstdCodec::new();
        let input = b"hello zstd codec world";
        let compressed = codec
            .compress(input, CompressionLevel::default())
            .expect("encode");
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .expect("decode");
        assert_eq!(decompressed, input);
    }
}
