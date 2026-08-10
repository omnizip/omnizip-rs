//! `ZstdCodec` — adapts the ZSTD encoder + decoder to the
//! `omnizip_codecs::Codec` trait.

#![forbid(unsafe_code)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

use crate::{decompress, ZstdDecoder, ZstdError};

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
        // Map the omnizip CompressionLevel (0-22) directly to the ZSTD
        // reference level (1-22), using the full cparams table from
        // `clevels.h`. This gives fine-grained level differentiation:
        // each level has its own (window_log, chain_log, hash_log,
        // search_log, min_match, target_length, strategy) tuple.
        //
        // Previously this collapsed 22 levels into just 5 ZstdLevel
        // enum values, losing the per-level parameter tuning.
        let zstd_level = level.as_u8().clamp(1, 22);
        crate::encoder::block::encode_frame_compressed(plaintext, zstd_level).map_err(|e| {
            OmnizipError::EncodeFailed {
                codec: CodecId::ZSTD,
                reason: e.to_string(),
            }
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

    fn default_fast_level(&self) -> u8 {
        1
    }
    fn default_balanced_level(&self) -> u8 {
        9
    }
    fn default_max_ratio_level(&self) -> u8 {
        19
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
