//! omnizip-zpaq — pure-Rust ZPAQ context-mixing archival codec.
//!
//! Phase 2 implementation: four prediction models (order-0, order-1,
//! order-2 byte-context, and a match model) are combined via an adaptive
//! logistic mixer ([`mixer::Mixer`]) that drives the binary arithmetic
//! coder. The container format wraps the coded bitstream with an 11-byte
//! header carrying the magic, version, model configuration id, and
//! uncompressed size.
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
//! Phase 2 uses a single fixed model portfolio for all levels; the level
//! parameter is currently accepted but does not switch models. Future
//! phases may select between model portfolios (e.g. faster order-0/1 only
//! at low levels).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod arithmetic;
pub mod container;
pub mod mixer;
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

/// Supported compression-level range for Phase 2. The single model
/// portfolio produces identical output for any level in this range; future
/// phases may select between portfolios.
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
        // Phase 2: a single model portfolio is used for all valid levels.
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
    fn round_trip_source_code_snippet() {
        // Real source code: varied byte distribution with mild repetition.
        let code = b"// Diverse source code stresses order-0/order-1 models.\n\
fn quicksort<T: Ord>(slice: &mut [T]) {\n    if slice.len() <= 1 { return; }\n    let pivot = partition(slice);\n    let (left, right) = slice.split_at_mut(pivot);\n    quicksort(left);\n    quicksort(&mut right[1..]);\n}\n\
fn partition<T: Ord>(slice: &mut [T]) -> usize {\n    let len = slice.len();\n    let pivot = len - 1;\n    let mut i = 0;\n    for j in 0..pivot {\n        if slice[j] <= slice[pivot] {\n            slice.swap(i, j);\n            i += 1;\n        }\n    }\n    slice.swap(i, pivot);\n    i\n}\n".repeat(4);
        let c = compress(&code);
        let d = decompress(&c).expect("decompress");
        assert_eq!(d, code);
        // Should achieve meaningful compression on real source code.
        assert!(
            c.len() < code.len(),
            "expected compression, got ratio {:.3}",
            c.len() as f64 / code.len() as f64
        );
    }

    #[test]
    fn round_trip_diverse_prose() {
        // Non-repetitive English prose — the regime Phase 2 targets.
        let prose = b"The art of compression lies in exploiting redundancy. \
Where bytes repeat, simple dictionary coders excel. Where statistical \
structure dominates, context models predict the next symbol from its \
neighbours. Arithmetic coding then converts each prediction into a \
fraction of a bit, approaching the entropy bound. Mixing several \
models, each sensitive to a different kind of regularity, sharpens \
the prediction further: an order-0 model captures marginal frequencies, \
order-1 captures bigram structure, and a match model exploits long \
repetitions. Logistic combination in the log-odds domain lets these \
diverse estimates add constructively.";
        let c = compress(prose);
        let d = decompress(&c).expect("decompress");
        assert_eq!(d, prose);
        assert!(
            c.len() < prose.len(),
            "phase2 should still compress diverse prose"
        );
    }

    #[test]
    fn determinism_same_input_same_output() {
        let text = b"deterministic compression is required for content addressing";
        let c1 = compress(text);
        let c2 = compress(text);
        assert_eq!(c1, c2, "non-deterministic output");
    }

    /// Sanity: Phase 2 ratio on the standard test text should beat the
    /// Phase 1 ratio (single order-2 model). The acceptance target is
    /// "better than ~53%" as called out in the task brief.
    #[test]
    fn phase2_ratio_beats_phase1_baseline_on_source_code() {
        // Real source code is the failure mode for Phase 1 (~53% reported).
        let code = b"fn main() {\n    let data = vec![1, 2, 3, 4, 5];\n    for x in &data {\n        println!(\"{}\", x);\n    }\n}\n\
struct Point { x: i32, y: i32 }\n\
impl Point {\n    fn new(x: i32, y: i32) -> Self { Self { x, y } }\n    fn distance(&self, other: &Self) -> f64 {\n        let dx = (self.x - other.x) as f64;\n        let dy = (self.y - other.y) as f64;\n        (dx * dx + dy * dy).sqrt()\n    }\n}\n".repeat(6);

        let c = compress(&code);
        let _ = decompress(&c).expect("decompress"); // round-trip sanity

        // Phase 1 baseline (reproduced here for direct comparison).
        let mut enc = crate::arithmetic::ArithmeticEncoder::new();
        let mut m1 = crate::model::Order2Model::new();
        for &b in &code {
            m1.encode_byte(b, &mut enc);
        }
        let phase1_bytes = enc.finish();

        let ratio_p1 = phase1_bytes.len() as f64 / code.len() as f64;
        let ratio_p2 = (c.len() - crate::container::HEADER_LEN) as f64 / code.len() as f64;
        eprintln!(
            "source code: phase1 ratio {:.4} ({} payload bytes); \
phase2 ratio {:.4} ({} payload bytes, {} total)",
            ratio_p1,
            phase1_bytes.len(),
            ratio_p2,
            c.len() - crate::container::HEADER_LEN,
            c.len()
        );
        assert!(
            ratio_p2 < ratio_p1,
            "phase2 ({ratio_p2:.4}) should beat phase1 ({ratio_p1:.4}) on source code"
        );
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
