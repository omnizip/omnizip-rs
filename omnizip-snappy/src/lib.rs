//! Pure-Rust Snappy codec.
//!
//! The encoder + decoder are implemented in-house from the Snappy
//! framing format description (`codec::encode` / `codec::decode`).
//! The `snap` crate remains as an optional dependency for callers
//! that want the upstream implementation; the in-house path is the
//! default for `SnappyCodec`.
//!
//! Snappy is Google's high-speed, low-ratio codec used in Parquet, ORC,
//! Avro, and `SQLite` WAL files. It has no compression levels; the encode
//! and decode paths are fixed.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod codec;

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// Snappy codec. No compression levels — encode and decode are fixed.
///
/// Uses the in-house encoder + decoder from [`codec`]. Output is
/// byte-compatible with `snap`/`snappy` reference tools (same wire
/// format); only the match-finder strategy differs.
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
        Ok(codec::encode(plaintext))
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::SNAPPY,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;
        let decoded = codec::decode(compressed).map_err(|reason| OmnizipError::DecodeFailed {
            codec: CodecId::SNAPPY,
            reason: format!("in-house decode failed: {reason}"),
        })?;
        if decoded.len() != expected_us {
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::SNAPPY,
                expected: expected_len,
                actual: decoded.len(),
            });
        }
        Ok(decoded)
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

    #[test]
    fn in_house_decodes_snap_encoded_output() {
        let data = b"the quick brown fox jumps over the lazy dog. ".repeat(20);
        let mut snap_enc = snap::raw::Encoder::new();
        let snap_compressed = snap_enc.compress_vec(&data).expect("snap encode");
        let decoded = SnappyCodec
            .decompress(&snap_compressed, data.len() as u32)
            .expect("in-house decode");
        assert_eq!(decoded, data);
    }

    #[test]
    fn snap_decodes_in_house_encoded_output() {
        let data = b"the quick brown fox jumps over the lazy dog. ".repeat(20);
        let compressed = SnappyCodec
            .compress(&data, CompressionLevel::default())
            .expect("in-house encode");
        let mut snap_dec = snap::raw::Decoder::new();
        let decoded = snap_dec.decompress_vec(&compressed).expect("snap decode");
        assert_eq!(decoded, data);
    }
}
