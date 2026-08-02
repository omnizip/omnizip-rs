//! omnizip-zpaq — pure-Rust ZPAQ context-mixing archival codec.
//!
//! Phase 1 implementation: a single order-2 byte-context adaptive
//! probability model drives a binary arithmetic coder. The container
//! format wraps the coded bitstream with an 11-byte header carrying the
//! magic, version, model configuration id, and uncompressed size.
//!
//! ## Public API
//!
//! - [`compress`] / [`decompress`] — free functions operating on byte slices.
//! - [`ZpaqCodec`] — implements the [`omnizip_codecs::Codec`] trait so the
//!   codec can be registered with a `CodecRegistry`.
//!
//! ## Determinism
//!
//! The codec is fully deterministic: identical inputs produce byte-
//! identical outputs, regardless of machine, Rust version, or run order.
//! This is required by omnizip-rs's content-addressed-storage consumer
//! (`LimniFS`, where `DropId = BLAKE3(plaintext)`).
//!
//! ## Compression level
//!
//! Phase 1 has a single fixed model; all levels produce identical output.
//! Higher levels will be wired in when the context mixer (Phase 2) lands.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod arithmetic;
pub mod container;
pub mod model;

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

pub use container::{compress_container, decompress_container};

/// Errors specific to the omnizip-zpaq codec.
#[derive(Debug)]
pub enum ZpaqError {
    /// Container header is malformed.
    Container(container::ContainerError),
}

impl std::fmt::Display for ZpaqError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Container(e) => write!(f, "zpaq container error: {e}"),
        }
    }
}

impl std::error::Error for ZpaqError {}

/// Compress `input` into a ZPAQ container.
///
/// Deterministic and allocation-bounded by the input size.
#[must_use]
pub fn compress(input: &[u8]) -> Vec<u8> {
    compress_container(input)
}

/// Decompress a ZPAQ container produced by [`compress`].
///
/// # Errors
///
/// Returns [`ZpaqError::Container`] on malformed input.
pub fn decompress(compressed: &[u8]) -> Result<Vec<u8>, ZpaqError> {
    decompress_container(compressed).map_err(ZpaqError::Container)
}

/// ZPAQ codec implementing [`Codec`].
///
/// # Codec id
///
/// The task specification requested id `0x0B`, but that value is already
/// assigned to `LIBDEFLATE` in `omnizip-codecs::CodecId`. We use the next
/// free id `0x000D` to avoid a collision. Update the `CodecId` constant
/// here (and in `omnizip-codecs`) if a different assignment is desired.
pub struct ZpaqCodec;

/// ZPAQ codec identifier. See [`ZpaqCodec`] for the rationale.
///
/// Note: this is defined locally because `omnizip-codecs` does not yet
/// export a `ZPAQ` constant. When the constant is added upstream, replace
/// this with `CodecId::ZPAQ`.
pub const ZPAQ_CODEC_ID: CodecId = CodecId::new(0x000D);

/// Supported compression-level range for Phase 1. The single fixed model
/// produces identical output for any level in this range.
const LEVEL_MIN: u8 = 0;
const LEVEL_MAX: u8 = 9;

impl Codec for ZpaqCodec {
    fn id(&self) -> CodecId {
        ZPAQ_CODEC_ID
    }

    fn name(&self) -> &'static str {
        "zpaq"
    }

    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        let lvl = level.as_u8();
        if lvl > LEVEL_MAX {
            return Err(OmnizipError::LevelOutOfRange {
                codec: ZPAQ_CODEC_ID,
                level: lvl,
                min: LEVEL_MIN,
                max: LEVEL_MAX,
            });
        }
        // Phase 1: level is currently ignored; the order-2 model is fixed.
        Ok(compress_container(plaintext))
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: ZPAQ_CODEC_ID,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;
        let out = decompress_container(compressed).map_err(|e| OmnizipError::DecodeFailed {
            codec: ZPAQ_CODEC_ID,
            reason: e.to_string(),
        })?;
        if out.len() != expected_us {
            return Err(OmnizipError::LengthMismatch {
                codec: ZPAQ_CODEC_ID,
                expected: expected_len,
                actual: out.len(),
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let c = compress(b"");
        let d = decompress(&c).expect("decompress");
        assert_eq!(d, b"");
    }

    #[test]
    fn round_trip_single_byte() {
        let c = compress(b"X");
        let d = decompress(&c).expect("decompress");
        assert_eq!(d, b"X");
    }

    #[test]
    fn round_trip_text() {
        let text = b"The quick brown fox jumps over the lazy dog. ".repeat(50);
        let c = compress(&text);
        let d = decompress(&c).expect("decompress");
        assert_eq!(d, text);
        assert!(c.len() < text.len(), "expected compression");
    }

    #[test]
    fn round_trip_binary() {
        let data: Vec<u8> = (0..1024).map(|i| (i * 7) as u8).collect();
        let c = compress(&data);
        let d = decompress(&c).expect("decompress");
        assert_eq!(d, data);
    }

    #[test]
    fn round_trip_ff_runs() {
        // Carry stress: long runs of 0xFF and 0x00 alternating.
        let mut data = Vec::new();
        for _ in 0..10 {
            data.extend_from_slice(&[0xFF; 256]);
            data.extend_from_slice(&[0x00; 256]);
            data.extend_from_slice(&[0xFF; 128]);
        }
        let c = compress(&data);
        let d = decompress(&c).expect("decompress");
        assert_eq!(d, data);
    }

    #[test]
    fn compresses_text_smaller_than_input() {
        let text = b"all good coders write tests, and all good tests cover code. ".repeat(40);
        let c = compress(&text);
        assert!(
            c.len() < text.len(),
            "ratio {:.3}: {} -> {}",
            c.len() as f64 / text.len() as f64,
            text.len(),
            c.len()
        );
    }

    #[test]
    fn determinism_same_input_same_output() {
        let text = b"deterministic compression is required for content addressing";
        let c1 = compress(text);
        let c2 = compress(text);
        assert_eq!(c1, c2, "non-deterministic output");
    }

    #[test]
    fn codec_trait_round_trip() {
        let codec = ZpaqCodec;
        let data = b"codec trait round trip".repeat(20);
        let compressed = codec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        let decompressed = codec
            .decompress(&compressed, data.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, data);
    }

    #[test]
    fn codec_trait_rejects_out_of_range_level() {
        let codec = ZpaqCodec;
        let result = codec.compress(b"x", CompressionLevel::new(LEVEL_MAX + 1));
        assert!(matches!(
            result,
            Err(OmnizipError::LevelOutOfRange { level, .. }) if level == LEVEL_MAX + 1
        ));
    }

    #[test]
    fn codec_trait_rejects_truncated_input() {
        let codec = ZpaqCodec;
        let result = codec.decompress(b"ZPA", 10);
        assert!(result.is_err());
    }

    #[test]
    fn codec_trait_rejects_length_mismatch() {
        let codec = ZpaqCodec;
        let data = b"hello";
        let compressed = codec
            .compress(data, CompressionLevel::default())
            .expect("compress");
        let wrong_len = data.len() as u32 + 1;
        let result = codec.decompress(&compressed, wrong_len);
        assert!(matches!(
            result,
            Err(OmnizipError::LengthMismatch { expected, actual, .. })
                if expected == wrong_len && actual == data.len()
        ));
    }
}
