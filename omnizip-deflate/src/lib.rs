//! Pure-Rust DEFLATE codec — wraps [`miniz_oxide`] behind the
//! [`omnizip_codecs::Codec`] trait.
//!
//! Produces zlib-framed RFC 1951 streams (2-byte zlib header + DEFLATE
//! body + Adler-32 checksum) decodable by any zlib decoder.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// DEFLATE codec. Levels 0–9 map to `miniz_oxide` levels.
pub struct DeflateCodec;

impl Codec for DeflateCodec {
    fn id(&self) -> CodecId {
        CodecId::DEFLATE
    }
    fn name(&self) -> &'static str {
        "deflate"
    }
    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        let miniz_level = clamp_level(level);
        Ok(miniz_oxide::deflate::compress_to_vec_zlib(
            plaintext,
            miniz_level,
        ))
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::DEFLATE,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;
        let result = miniz_oxide::inflate::decompress_to_vec_zlib(compressed).map_err(|e| {
            OmnizipError::DecodeFailed {
                codec: CodecId::DEFLATE,
                reason: format!("deflate decompress failed: {e:?}"),
            }
        })?;
        if result.len() != expected_us {
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::DEFLATE,
                expected: expected_len,
                actual: result.len(),
            });
        }
        Ok(result)
    }
}

fn clamp_level(level: CompressionLevel) -> u8 {
    match level.as_u8() {
        0 => 1,
        n if n <= 9 => n,
        _ => 6,
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_text() {
        let data = b"The quick brown fox. ".repeat(500);
        let compressed = DeflateCodec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        let decompressed = DeflateCodec
            .decompress(&compressed, data.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, data);
    }

    #[test]
    fn round_trip_at_each_level() {
        let data = b"Lorem ipsum dolor sit amet. ".repeat(200);
        for level in 0..=9u8 {
            let compressed = DeflateCodec
                .compress(&data, CompressionLevel::new(level))
                .expect("compress");
            let decompressed = DeflateCodec
                .decompress(&compressed, data.len() as u32)
                .expect("decompress");
            assert_eq!(decompressed, data, "level {level} round-trip");
        }
    }

    #[test]
    fn rejects_truncated_input() {
        let result = DeflateCodec.decompress(b"\x78\x9c\x00", 100);
        assert!(result.is_err());
    }
}
