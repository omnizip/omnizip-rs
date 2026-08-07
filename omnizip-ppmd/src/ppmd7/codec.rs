//! PPMd7 codec: wraps the PPMd7 model in a container format and
//! implements the `Codec` trait.
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
//! The magic distinguishes PPMd7 streams from PPMd8's `b"PPD8\0"`.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::cast_precision_loss,
    clippy::doc_markdown
)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

use super::model::PpmModel;
use super::{Ppmd7Error, PPMD7_CODEC_ID, PPMD7_MAGIC};
use omnizip_codecs::arith::{ArithDecoder, ArithEncoder};

/// Minimum context order.
pub const MIN_ORDER: u8 = 1;
/// Maximum context order.
pub const MAX_ORDER: u8 = 16;
/// Default context order when none specified.
pub const DEFAULT_ORDER: u8 = 4;

/// Default memory budget (~80 MB) used by [`compress`] and [`compress_default`].
/// Override with [`compress_with_budget`].
pub const DEFAULT_MEMORY_BUDGET_BYTES: usize = 80 * 1024 * 1024;

/// Compress `input` with the given `max_order`. Returns the container
/// bytes (magic + order + size + bitstream).
///
/// Uses the default memory budget ([`DEFAULT_MEMORY_BUDGET_BYTES`],
/// ~80 MB). Call [`compress_with_budget`] to override.
///
/// # Errors
///
/// Returns [`Ppmd7Error::InvalidOrder`] if `max_order` is outside `[1, 16]`.
///
/// # Memory budget
///
/// The model uses a fixed-size context table (~80 MB by default)
/// plus a sliding-window history of `max_order` bytes. Memory is
/// **bounded regardless of input size** — a gigabyte input still
/// uses ~80 MB. Override with [`compress_with_budget`].
pub fn compress(input: &[u8], max_order: u8) -> Result<Vec<u8>, Ppmd7Error> {
    compress_with_budget(input, max_order, DEFAULT_MEMORY_BUDGET_BYTES)
}

/// Compress `input` with an explicit memory budget in bytes.
///
/// Larger budgets allow more contexts to be tracked, improving the
/// compression ratio on inputs with many distinct contexts. Smaller
/// budgets trade ratio for memory.
///
/// # Errors
///
/// Returns [`Ppmd7Error::InvalidOrder`] if `max_order` is outside `[1, 16]`.
pub fn compress_with_budget(
    input: &[u8],
    max_order: u8,
    memory_budget_bytes: usize,
) -> Result<Vec<u8>, Ppmd7Error> {
    validate_order(max_order)?;

    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(PPMD7_MAGIC);
    out.push(max_order);
    let uncompressed_size =
        u32::try_from(input.len()).map_err(|_| Ppmd7Error::TooLarge(input.len()))?;
    out.extend_from_slice(&uncompressed_size.to_le_bytes());

    if input.is_empty() {
        return Ok(out);
    }

    let mut model = PpmModel::with_memory_budget(usize::from(max_order), memory_budget_bytes);
    {
        let mut enc = ArithEncoder::new();
        for &b in input {
            model.encode_byte(&mut enc, b);
        }
        enc.flush(&mut out);
    }
    Ok(out)
}

/// Decompress a container produced by [`compress`] or [`compress_with_budget`].
/// `expected_len` is a cross-check: the container carries its own size
/// field, and this function verifies both agree and that the decoded
/// length matches.
///
/// # Errors
///
/// Returns [`Ppmd7Error::BadMagic`], [`Ppmd7Error::Corrupt`], or
/// [`Ppmd7Error::InvalidOrder`] on structural problems.
pub fn decompress(compressed: &[u8], expected_len: usize) -> Result<Vec<u8>, Ppmd7Error> {
    decompress_with_budget(compressed, expected_len, DEFAULT_MEMORY_BUDGET_BYTES)
}

/// Decompress with an explicit memory budget. The budget must be at
/// least as large as the one used to compress (otherwise the model
/// may diverge from the encoder).
///
/// # Errors
///
/// Same as [`decompress`].
pub fn decompress_with_budget(
    compressed: &[u8],
    expected_len: usize,
    memory_budget_bytes: usize,
) -> Result<Vec<u8>, Ppmd7Error> {
    if compressed.len() < 10 {
        return Err(Ppmd7Error::Corrupt("header too short".into()));
    }
    if &compressed[0..5] != PPMD7_MAGIC {
        return Err(Ppmd7Error::BadMagic);
    }
    let max_order = compressed[5];

    let size = u32::from_le_bytes([compressed[6], compressed[7], compressed[8], compressed[9]]);
    let size = usize::try_from(size).map_err(|_| Ppmd7Error::Corrupt("size overflow".into()))?;
    if size != expected_len {
        return Err(Ppmd7Error::Corrupt(format!(
            "size mismatch: header says {size}, caller expects {expected_len}"
        )));
    }

    validate_order(max_order)?;

    let mut out = Vec::with_capacity(size);
    if size == 0 {
        return Ok(out);
    }

    let bitstream = &compressed[10..];
    let mut model = PpmModel::with_memory_budget(usize::from(max_order), memory_budget_bytes);
    let mut dec = ArithDecoder::new(bitstream);
    for _ in 0..size {
        out.push(model.decode_byte(&mut dec));
    }
    Ok(out)
}

fn validate_order(max_order: u8) -> Result<(), Ppmd7Error> {
    if max_order == 0 || max_order > MAX_ORDER {
        Err(Ppmd7Error::InvalidOrder(max_order))
    } else {
        Ok(())
    }
}

/// Backwards-compat alias for the default order.
pub const DEFAULT_MAX_ORDER: u8 = DEFAULT_ORDER;

/// Compress with the default order-4 model.
///
/// # Errors
///
/// Only fails on internal error (the default order is always valid).
pub fn compress_default(input: &[u8]) -> Result<Vec<u8>, Ppmd7Error> {
    compress(input, DEFAULT_ORDER)
}

/// PPMd7 codec struct, implementing [`Codec`].
///
/// The [`CompressionLevel`] is mapped to a `max_order`:
/// level 0 ⇒ order 2, level 6 (default) ⇒ order 4, level 22 (best) ⇒
/// order 8. This is a heuristic; PPMd's quality scales weakly with order
/// beyond ~6 but memory grows fast.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ppmd7Codec;

impl Ppmd7Codec {
    /// Create a `Ppmd7Codec`.
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

impl Codec for Ppmd7Codec {
    fn id(&self) -> CodecId {
        PPMD7_CODEC_ID
    }

    fn name(&self) -> &'static str {
        "ppmd7"
    }

    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        let order = Self::level_to_order(level);
        compress(plaintext, order).map_err(|e| OmnizipError::EncodeFailed {
            codec: self.id(),
            reason: e.to_string(),
        })
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let expected = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: self.id(),
            reason: format!("expected_len {expected_len} overflows usize"),
        })?;
        decompress(compressed, expected).map_err(|e| OmnizipError::Corrupt {
            codec: self.id(),
            reason: e.to_string(),
        })
    }
}

/// Reusable PPMd7 compressor that caches the context-tree allocation
/// across calls. Mirrors `omnizip_zstd::ZstdCompressor`.
///
/// ## When to use
///
/// For batch workloads with many small inputs at the same `max_order`,
/// `PpmdCompressor` eliminates the per-call context-tree allocation
/// (which dominates wall-time for inputs < 4 KiB).
///
/// Each call resets adaptation via `PpmModel::reset` but reuses the
/// underlying `Vec`s. Output is byte-identical to
/// [`Ppmd7Codec::compress`].
pub struct PpmdCompressor {
    model: PpmModel,
    last_order: u8,
}

impl Default for PpmdCompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl PpmdCompressor {
    /// Construct a reusable PPMd7 compressor with the default memory
    /// budget (`DEFAULT_MEMORY_BUDGET_BYTES`).
    #[must_use]
    pub fn new() -> Self {
        Self::with_order(4)
    }

    /// Construct with a specific `max_order`. Subsequent calls can use
    /// a different order; the model is rebuilt only on order change.
    #[must_use]
    pub fn with_order(max_order: u8) -> Self {
        let order = usize::from(max_order.clamp(1, 16));
        Self {
            model: PpmModel::with_memory_budget(order, DEFAULT_MEMORY_BUDGET_BYTES),
            last_order: max_order,
        }
    }

    /// Compress `input` with the given level. If the level maps to a
    /// different `max_order` than the previous call, the model is
    /// reallocated. Otherwise it's reset and reused.
    pub fn compress(
        &mut self,
        input: &[u8],
        level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        let order = Ppmd7Codec::level_to_order(level);
        if order != self.last_order {
            self.model =
                PpmModel::with_memory_budget(usize::from(order), DEFAULT_MEMORY_BUDGET_BYTES);
            self.last_order = order;
        } else {
            self.model.reset();
        }

        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(PPMD7_MAGIC);
        out.push(order);
        let uncompressed_size = u32::try_from(input.len()).map_err(|_| OmnizipError::Corrupt {
            codec: PPMD7_CODEC_ID,
            reason: format!("input len {} overflows u32", input.len()),
        })?;
        out.extend_from_slice(&uncompressed_size.to_le_bytes());

        if input.is_empty() {
            return Ok(out);
        }

        {
            let mut enc = ArithEncoder::new();
            for &b in input {
                self.model.encode_byte(&mut enc, b);
            }
            enc.flush(&mut out);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "The quick brown fox jumps over the lazy dog. \
        Pack my box with five dozen liquor jugs. \
        She sells seashells by the seashore. \
        Peter Piper picked a peck of pickled peppers. \
        How much wood would a woodchuck chuck if a woodchuck could chuck wood?";

    fn round_trip(input: &[u8], order: u8) {
        let compressed = compress(input, order).expect("compress");
        let decompressed = decompress(&compressed, input.len()).expect("decompress");
        assert_eq!(decompressed, input, "round-trip failed at order {order}");
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
        assert_eq!(compressed.len(), 10);
        let decompressed = decompress(&compressed, 0).expect("decompress empty");
        assert!(decompressed.is_empty());
    }
    #[test]
    fn round_trip_single_byte() {
        round_trip(b"X", 4);
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
        let big: Vec<u8> = TEXT.bytes().cycle().take(20_000).collect();
        round_trip(&big, 4);
    }

    #[test]
    fn ppmd_compressor_matches_one_shot_api() {
        // Same input + same level → byte-identical output between the
        // reusable `PpmdCompressor` and the one-shot `Ppmd7Codec`.
        let input: Vec<u8> = TEXT.bytes().cycle().take(2048).collect();
        let one_shot = Ppmd7Codec
            .compress(&input, CompressionLevel::default())
            .expect("one-shot");

        let mut comp = PpmdCompressor::new();
        let reusable = comp
            .compress(&input, CompressionLevel::default())
            .expect("reusable");

        assert_eq!(
            one_shot, reusable,
            "PpmdCompressor must produce identical output to Ppmd7Codec"
        );
    }

    #[test]
    fn ppmd_compressor_round_trips_across_calls() {
        // Multiple calls in sequence should each round-trip cleanly.
        let mut comp = PpmdCompressor::new();
        for input in ["foo", "bar", "baz", "the quick brown fox"] {
            let c = comp
                .compress(input.as_bytes(), CompressionLevel::default())
                .expect("compress");
            let d = decompress(&c, input.len()).expect("decompress");
            assert_eq!(d.as_slice(), input.as_bytes());
        }
    }
    #[test]
    fn round_trip_binary_zero_inclusive() {
        let mut data = Vec::new();
        for _ in 0..50 {
            data.extend_from_slice(&[0u8; 5]);
            data.push(255);
        }
        round_trip(&data, 4);
    }
    #[test]
    fn compressed_text_is_smaller() {
        let big: Vec<u8> = TEXT.bytes().cycle().take(10_000).collect();
        let compressed = compress(&big, 4).expect("compress");
        assert!(compressed.len() < big.len());
    }
    #[test]
    fn determinism_same_input_same_output() {
        let input = TEXT.as_bytes();
        let a = compress(input, 4).expect("a");
        let b = compress(input, 4).expect("b");
        assert_eq!(a, b);
    }
    #[test]
    fn codec_trait_round_trip() {
        let codec = Ppmd7Codec::new();
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
        let codec = Ppmd7Codec::new();
        assert_eq!(codec.id(), CodecId::PPMD7);
        assert_eq!(codec.id().as_u16(), 0x0008);
        assert_eq!(codec.name(), "ppmd7");
    }
    #[test]
    fn rejects_invalid_order() {
        assert!(compress(b"hi", 0).is_err());
        assert!(compress(b"hi", 17).is_err());
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
        assert!(decompress(&compressed, 10).is_err());
    }
    #[test]
    fn level_mapping_is_monotonic() {
        let o0 = Ppmd7Codec::level_to_order(CompressionLevel::new(0));
        let o6 = Ppmd7Codec::level_to_order(CompressionLevel::new(6));
        let o22 = Ppmd7Codec::level_to_order(CompressionLevel::new(22));
        assert!(o0 <= o6);
        assert!(o6 <= o22);
    }
    #[test]
    fn round_trip_large_input() {
        let input: Vec<u8> = (0..100_000u64).map(|i| ((i * 7919) % 251) as u8).collect();
        let compressed = compress(&input, 4).expect("compress");
        let out = decompress(&compressed, input.len()).expect("decompress");
        assert_eq!(out, input);
    }
}
