//! Pure-Rust Snappy codec — wraps the [`snap`](https://crates.io/crates/snap)
//! crate (the standard pure-Rust Snappy implementation) behind the
//! [`omnizip_codecs::Codec`] trait.
//!
//! Snappy is Google's high-speed, low-ratio codec used in Parquet, ORC,
//! Avro, and `SQLite` WAL files. It has no compression levels; the encode
//! and decode paths are fixed.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// Snappy codec. No compression levels — encode and decode are fixed.
pub struct SnappyCodec;

impl Codec for SnappyCodec {
    fn id(&self) -> CodecId {
        CodecId::SNAPPY
    }

    fn name(&self) -> &'static str {
        "snappy"
    }

    fn compress(
        &self,
        plaintext: &[u8],
        _level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        let mut encoder = snap::raw::Encoder::new();
        encoder
            .compress_vec(plaintext)
            .map_err(|e| OmnizipError::EncodeFailed {
                codec: CodecId::SNAPPY,
                reason: format!("snap compress failed: {e}"),
            })
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::SNAPPY,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;
        let mut decoder = snap::raw::Decoder::new();
        let result =
            decoder
                .decompress_vec(compressed)
                .map_err(|e| OmnizipError::DecodeFailed {
                    codec: CodecId::SNAPPY,
                    reason: format!("snap decompress failed: {e}"),
                })?;
        if result.len() != expected_us {
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::SNAPPY,
                expected: expected_len,
                actual: result.len(),
            });
        }
        Ok(result)
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_text() {
        let data = b"The quick brown fox jumps over the lazy dog. ".repeat(100);
        let compressed = SnappyCodec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        let decompressed = SnappyCodec
            .decompress(&compressed, data.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, data);
    }

    #[test]
    fn round_trip_binary() {
        let data: Vec<u8> = (0..10_000u32).map(|i| i as u8).collect();
        let compressed = SnappyCodec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        let decompressed = SnappyCodec
            .decompress(&compressed, data.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, data);
    }

    #[test]
    fn compresses_repetitive_data() {
        let data = vec![0x41u8; 10_000];
        let compressed = SnappyCodec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        assert!(compressed.len() < data.len());
    }

    #[test]
    fn rejects_truncated_input() {
        let result = SnappyCodec.decompress(b"\xff\x00\x00", 100);
        assert!(result.is_err());
    }
}
