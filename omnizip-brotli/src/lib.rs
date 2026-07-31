//! Pure-Rust Brotli codec — wraps the [`brotli`] crate (by Daniel Reiter
//! Horn, the format's original author) behind the [`omnizip_codecs::Codec`]
//! trait.
//!
//! Brotli is the highest-ratio pure-Rust codec in the registry at quality
//! 11. It outperforms ZSTD and LZMA on text and web content.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use std::io::Cursor;

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// Brotli quality 11 (the reference encoder's maximum).
const DEFAULT_QUALITY: i32 = 11;

/// Brotli codec. Encodes at quality `level` (0–11); default 11.
pub struct BrotliCodec;

impl Codec for BrotliCodec {
    fn id(&self) -> CodecId {
        CodecId::BROTLI
    }
    fn name(&self) -> &'static str {
        "brotli"
    }
    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        let quality = i32::from(level.as_u8().min(11));
        let params = brotli::enc::backward_references::BrotliEncoderParams {
            quality,
            ..Default::default()
        };
        let mut output = Vec::new();
        brotli::BrotliCompress(&mut Cursor::new(plaintext), &mut output, &params).map_err(|e| {
            OmnizipError::EncodeFailed {
                codec: CodecId::BROTLI,
                reason: format!("brotli compress (quality {quality}) failed: {e}"),
            }
        })?;
        Ok(output)
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::BROTLI,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;
        let mut output = Vec::with_capacity(expected_us);
        brotli::BrotliDecompress(&mut Cursor::new(compressed), &mut output).map_err(|e| {
            OmnizipError::DecodeFailed {
                codec: CodecId::BROTLI,
                reason: format!("brotli decompress failed: {e}"),
            }
        })?;
        if output.len() != expected_us {
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::BROTLI,
                expected: expected_len,
                actual: output.len(),
            });
        }
        Ok(output)
    }
}

/// The default quality used when callers don't specify one.
#[must_use]
pub fn default_quality() -> i32 {
    DEFAULT_QUALITY
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_at_quality_11() {
        let data = b"The quick brown fox jumps over the lazy dog. ".repeat(200);
        let compressed = BrotliCodec
            .compress(&data, CompressionLevel::new(11))
            .expect("compress");
        let decompressed = BrotliCodec
            .decompress(&compressed, data.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, data);
    }

    #[test]
    fn q11_beats_q0_on_text() {
        let data = b"The quick brown fox. ".repeat(5_000);
        let q11 = BrotliCodec
            .compress(&data, CompressionLevel::new(11))
            .expect("q11");
        let q0 = BrotliCodec
            .compress(&data, CompressionLevel::new(0))
            .expect("q0");
        assert!(
            q11.len() < q0.len(),
            "brotli q11 ({}) should produce smaller output than q0 ({}) on text",
            q11.len(),
            q0.len()
        );
    }

    #[test]
    fn rejects_truncated_input() {
        let result = BrotliCodec.decompress(b"\x00\x00\x00", 100);
        assert!(result.is_err());
    }
}
