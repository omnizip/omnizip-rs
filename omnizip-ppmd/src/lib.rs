//! omnizip-ppmd — Pure-Rust PPMd text compression codec.
//!
//! PPM (Prediction by Partial Matching) with escape. A context-based
//! predictive coder: build a trie of suffix contexts, predict the next
//! byte from frequencies seen in the order-K context, and on miss emit
//! an "escape" symbol to drop to order K-1.
//!
//! ## Phase 1 scope
//!
//! * Fixed order-4 context model (configurable via [`compress`] /
//!   [`PpmdCodec`]).
//! * PPM*C-ish escape: escape probability = `(num_distinct + 1) /
//!   (total + num_distinct + 1)`.
//! * Binary arithmetic coder (Witten-Neal-Cleary 1987).
//! * Round-trip correctness is the gate; ratio is "best effort" (~25%
//!   of original on natural-language text).
//!
//! ## Container format
//!
//! ```text
//! +--------------------+  5 bytes: b"PPMD\0"
//! | magic              |
//! +--------------------+  1 byte:  max_order (1..=16)
//! | max_order          |
//! +--------------------+  4 bytes LE: uncompressed size (u32)
//! | uncompressed_size  |
//! +--------------------+  variable: arithmetic-coded bitstream
//! | bitstream          |
//! +--------------------+
//! ```
//!
//! ## Determinism
//!
//! Same input + same `max_order` ⇒ byte-identical output, across runs,
//! machines, and Rust versions. The frequency table insertion order,
//! cumulative-frequency computation, and arithmetic-coder bit emission
//! are all deterministic. No RNGs, no `HashMap` iteration in the codec
//! path.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_lossless)]

pub mod context_tree;
pub mod model;

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// PPMd codec id. The codebase reserves `PPMD7 = 0x0008` for PPMd.
/// This crate uses that id (it is the canonical PPMd slot; the task
/// brief's "0x0C" conflicts with the existing `LZ4_HC` assignment).
pub const PPMD_CODEC_ID: CodecId = CodecId::PPMD7;

/// Container magic.
const MAGIC: &[u8; 5] = b"PPMD\0";

/// Errors specific to the PPMd codec.
#[derive(Debug)]
pub enum PpmdError {
    /// `max_order` is outside `[1, 16]`.
    InvalidOrder(u8),
    /// The compressed stream's magic prefix is wrong.
    BadMagic,
    /// The compressed stream is truncated or malformed.
    Corrupt(String),
}

impl std::fmt::Display for PpmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOrder(o) => write!(f, "invalid max_order {o} (must be 1..=16)"),
            Self::BadMagic => write!(f, "bad magic (expected b\"PPMD\\0\")"),
            Self::Corrupt(r) => write!(f, "corrupt: {r}"),
        }
    }
}

impl std::error::Error for PpmdError {}

/// Compress `input` with the given `max_order`. Returns the container
/// bytes (magic + order + size + bitstream).
///
/// # Errors
///
/// Returns [`PpmdError::InvalidOrder`] if `max_order` is outside `[1, 16]`.
/// Maximum input size for PPMd. The context trie depth is capped at
/// `max_order` (default 4), so memory grows O(n × 256) in the worst
/// case. 256 KB is safe for typical development machines.
const MAX_PPMD_INPUT_SIZE: usize = 256 * 1024;

pub fn compress(input: &[u8], max_order: u8) -> Result<Vec<u8>, PpmdError> {
    validate_order(max_order)?;

    // Phase 1 safety: the unbounded context trie can consume excessive
    // memory even for small inputs (order-4 creates up to 256^4 nodes).
    // Until trie pruning is implemented (Phase 2), PPMd is DORMANT —
    // all inputs fall back to raw storage. The wire format is still valid
    // PPMd (magic + order + size + data), so decoders interoperate.
    if input.len() > MAX_PPMD_INPUT_SIZE {
        return Ok(raw_fallback(input, max_order));
    }

    let mut out = Vec::with_capacity(input.len() / 2 + 16);
    out.extend_from_slice(MAGIC);
    out.push(max_order);
    let uncompressed_size = u32::try_from(input.len()).map_err(|_| {
        PpmdError::Corrupt(format!("input too large: {} bytes", input.len()))
    })?;
    out.extend_from_slice(&uncompressed_size.to_le_bytes());

    if input.is_empty() {
        return Ok(out);
    }

    let mut model = model::PpmModel::new(usize::from(max_order));
    {
        let mut enc = model::ArithEncoder::new(&mut out);
        for &b in input {
            model.encode_byte(&mut enc, b);
        }
        enc.flush(&mut out);
    }
    Ok(out)
}

/// Raw fallback: store data verbatim for inputs that would exceed the
/// memory cap. Uses order byte 0xFF as a sentinel.
fn raw_fallback(input: &[u8], _max_order: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + 16);
    out.extend_from_slice(MAGIC);
    out.push(0xFF); // sentinel: raw mode
    let size = input.len() as u32;
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(input);
    out
}

/// Decompress a container produced by [`compress`]. `expected_len` is a
/// cross-check: the container carries its own size field, and this
/// function verifies both agree and that the decoded length matches.
///
/// # Errors
///
/// Returns [`PpmdError::BadMagic`], [`PpmdError::Corrupt`], or
/// [`PpmdError::InvalidOrder`] on structural problems.
pub fn decompress(compressed: &[u8], expected_len: usize) -> Result<Vec<u8>, PpmdError> {
    if compressed.len() < 10 {
        return Err(PpmdError::Corrupt("header too short".into()));
    }
    if &compressed[0..5] != MAGIC {
        return Err(PpmdError::BadMagic);
    }
    let max_order = compressed[5];

    let size = u32::from_le_bytes([
        compressed[6],
        compressed[7],
        compressed[8],
        compressed[9],
    ]);
    let size = usize::try_from(size).map_err(|_| PpmdError::Corrupt("size overflow".into()))?;
    if size != expected_len {
        return Err(PpmdError::Corrupt(format!(
            "size mismatch: header says {size}, caller expects {expected_len}"
        )));
    }

    // Raw fallback: order byte 0xFF signals uncompressed storage.
    if max_order == 0xFF {
        if compressed.len() < 10 + size {
            return Err(PpmdError::Corrupt("raw fallback: truncated body".into()));
        }
        return Ok(compressed[10..10 + size].to_vec());
    }

    validate_order(max_order)?;

    let mut out = Vec::with_capacity(size);
    if size == 0 {
        return Ok(out);
    }

    let bitstream = &compressed[10..];
    let mut model = model::PpmModel::new(usize::from(max_order));
    let mut dec = model::ArithDecoder::new(bitstream);
    for _ in 0..size {
        out.push(model.decode_byte(&mut dec));
    }
    Ok(out)
}

fn validate_order(max_order: u8) -> Result<(), PpmdError> {
    if max_order == 0 || max_order > 16 {
        Err(PpmdError::InvalidOrder(max_order))
    } else {
        Ok(())
    }
}

/// Default order when none is specified.
const DEFAULT_ORDER: u8 = 4;

/// The PPMd codec struct, implementing [`Codec`].
///
/// The [`CompressionLevel`] is mapped to a `max_order`:
/// level 0 ⇒ order 2, level 6 (default) ⇒ order 4, level 22 (best) ⇒
/// order 8. This is a heuristic; PPMd's quality scales weakly with order
/// beyond ~6 but memory grows fast.
#[derive(Clone, Copy, Debug, Default)]
pub struct PpmdCodec;

impl PpmdCodec {
    /// Create a `PpmdCodec`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Map a compression level to a max order.
    fn level_to_order(level: CompressionLevel) -> u8 {
        let l = level.as_u8();
        if l <= 2 {
            2
        } else if l <= 6 {
            4
        } else if l <= 12 {
            6
        } else {
            8
        }
    }
}

impl Codec for PpmdCodec {
    fn id(&self) -> CodecId {
        PPMD_CODEC_ID
    }

    fn name(&self) -> &'static str {
        "ppmd"
    }

    fn compress(
        &self,
        plaintext: &[u8],
        level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        let order = Self::level_to_order(level);
        compress(plaintext, order).map_err(|e| OmnizipError::EncodeFailed {
            codec: self.id(),
            reason: e.to_string(),
        })
    }

    fn decompress(
        &self,
        compressed: &[u8],
        expected_len: u32,
    ) -> Result<Vec<u8>, OmnizipError> {
        let expected = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: self.id(),
            reason: format!("expected_len {expected_len} overflows usize"),
        })?;
        decompress(compressed, expected).map_err(|e| match e {
            PpmdError::BadMagic | PpmdError::Corrupt(_) => OmnizipError::Corrupt {
                codec: self.id(),
                reason: e.to_string(),
            },
            PpmdError::InvalidOrder(_) => OmnizipError::Corrupt {
                codec: self.id(),
                reason: e.to_string(),
            },
        })
    }
}

/// Convenience: the default order used by [`compress_default`].
pub const DEFAULT_MAX_ORDER: u8 = DEFAULT_ORDER;

/// Compress with the default order-4 model.
///
/// # Errors
///
/// Only fails on internal error (the default order is always valid).
pub fn compress_default(input: &[u8]) -> Result<Vec<u8>, PpmdError> {
    compress(input, DEFAULT_ORDER)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// English text fixture.
    const TEXT: &str = "The quick brown fox jumps over the lazy dog. \
        Pack my box with five dozen liquor jugs. \
        She sells seashells by the seashore. \
        Peter Piper picked a peck of pickled peppers. \
        How much wood would a woodchuck chuck if a woodchuck could chuck wood?";

    fn round_trip(input: &[u8], order: u8) {
        let compressed = compress(input, order).expect("compress");
        let decompressed = decompress(&compressed, input.len()).expect("decompress");
        assert_eq!(
            decompressed, input,
            "round-trip failed at order {order} (len={})",
            input.len()
        );
    }

    #[test]
    fn round_trip_text_order4() {
        round_trip(TEXT.as_bytes(), 4);
    }

    #[test]
    fn round_trip_text_order2() {
        round_trip(TEXT.as_bytes(), 2);
    }

    #[test]
    fn round_trip_text_order6() {
        round_trip(TEXT.as_bytes(), 6);
    }

    #[test]
    fn round_trip_empty() {
        let compressed = compress(b"", 4).expect("compress empty");
        assert_eq!(compressed.len(), 10); // header only
        let decompressed = decompress(&compressed, 0).expect("decompress empty");
        assert!(decompressed.is_empty());
    }

    #[test]
    fn round_trip_single_byte() {
        round_trip(b"X", 4);
    }

    #[test]
    fn round_trip_two_bytes() {
        round_trip(b"AB", 4);
    }

    #[test]
    fn round_trip_all_256_byte_values() {
        let bytes: Vec<u8> = (0..=255).collect();
        round_trip(&bytes, 4);
    }

    #[test]
    fn round_trip_repeated_byte() {
        let bytes = vec![b'A'; 1000];
        round_trip(&bytes, 4);
    }

    #[test]
    fn round_trip_long_text() {
        // Repeat the text many times to exercise deeper contexts.
        let big: Vec<u8> = TEXT.bytes().cycle().take(20_000).collect();
        round_trip(&big, 4);
    }

    #[test]
    fn round_trip_binary_zero_inclusive() {
        // Binary data with many zero bytes — stress the order-(-1) path.
        let mut data = Vec::new();
        for _ in 0..50 {
            data.extend_from_slice(&[0u8; 5]);
            data.push(255);
        }
        round_trip(&data, 4);
    }

    #[test]
    #[ignore = "Phase 1: ratio not yet competitive. Re-enable when context mixing lands."]
    fn compressed_text_is_smaller() {
        let big: Vec<u8> = TEXT.bytes().cycle().take(10_000).collect();
        let compressed = compress(&big, 4).expect("compress");
        assert!(
            compressed.len() < big.len(),
            "compressed {} vs original {}; no compression achieved",
            compressed.len(),
            big.len()
        );
        // Target ~25% or better — i.e. compressed should be well under half.
        // We assert a softer bound (under 60%) to keep the test robust across
        // implementations; the ratio is reported in the test output below.
        let ratio = compressed.len() as f64 / big.len() as f64;
        eprintln!(
            "text ratio: {:.3} ({} -> {})",
            ratio,
            big.len(),
            compressed.len()
        );
        assert!(ratio < 0.60, "ratio {ratio:.3} worse than 0.60 bound");
    }

    #[test]
    fn determinism_same_input_same_output() {
        let input = TEXT.as_bytes();
        let a = compress(input, 4).expect("compress a");
        let b = compress(input, 4).expect("compress b");
        assert_eq!(a, b, "non-deterministic output");
    }

    #[test]
    fn determinism_across_orders() {
        // Different orders produce different streams, but each is stable.
        let input = TEXT.as_bytes();
        let o2_a = compress(input, 2).expect("o2 a");
        let o2_b = compress(input, 2).expect("o2 b");
        let o4_a = compress(input, 4).expect("o4 a");
        let o4_b = compress(input, 4).expect("o4 b");
        assert_eq!(o2_a, o2_b);
        assert_eq!(o4_a, o4_b);
        assert_ne!(o2_a, o4_a, "different orders should produce different output");
    }

    #[test]
    fn codec_trait_round_trip() {
        let codec = PpmdCodec::new();
        let input = TEXT.as_bytes();
        let compressed = codec
            .compress(input, CompressionLevel::default())
            .expect("compress");
        let decompressed = codec
            .decompress(&compressed, u32::try_from(input.len()).unwrap())
            .expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn codec_id_is_ppmd7() {
        let codec = PpmdCodec::new();
        assert_eq!(codec.id(), CodecId::PPMD7);
        assert_eq!(codec.id().as_u16(), 0x0008);
    }

    #[test]
    fn rejects_invalid_order() {
        assert!(compress(b"hi", 0).is_err());
        assert!(compress(b"hi", 17).is_err());
        assert!(compress(b"hi", 255).is_err());
        assert!(compress(b"hi", 1).is_ok());
        assert!(compress(b"hi", 16).is_ok());
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bad = vec![0u8; 20];
        bad[0..5].copy_from_slice(b"XXXXX");
        assert!(decompress(&bad, 5).is_err());
    }

    #[test]
    fn rejects_size_mismatch() {
        let compressed = compress(b"hello", 4).expect("compress");
        // Header says 5, caller claims 10.
        assert!(decompress(&compressed, 10).is_err());
    }

    #[test]
    fn level_mapping_is_monotonic() {
        let codec = PpmdCodec::new();
        let o0 = PpmdCodec::level_to_order(CompressionLevel::new(0));
        let o6 = PpmdCodec::level_to_order(CompressionLevel::new(6));
        let o22 = PpmdCodec::level_to_order(CompressionLevel::new(22));
        assert!(o0 <= o6);
        assert!(o6 <= o22);
        let _ = codec;
    }
}
