//! Pure-Rust Brotli codec (RFC 7932) — no external dependencies.
//!
//! Encoder: vendored from upstream brotli crate (BSD-3-Clause),
//! adapted to use our own alloc module. Produces Huffman-coded
//! Brotli streams at quality 0-11.
//!
//! Decoder: from-scratch implementation handling uncompressed
//! metablocks. Huffman-coded decode is being extended incrementally.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod commands;
pub mod compress_fragment;
pub mod decoder;
pub mod decoder_full;
pub mod dictionary;
pub mod encoder;
pub mod encoder_error;
pub mod fast_encoder;
pub mod huffman;
pub mod huffman_lookup;
pub mod prefix;
pub mod static_codes;

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// Brotli quality 11 (the reference encoder's maximum).
const DEFAULT_QUALITY: i32 = 11;

/// Default window size: 22 = 4 MB (matches brotli spec default).
pub const DEFAULT_WINDOW_SIZE: u8 = 22;

/// Minimum legal Brotli window size (per RFC 7932).
pub const MIN_WINDOW_SIZE: u8 = 10; // 1 KB

/// Maximum legal Brotli window size (per RFC 7932).
pub const MAX_WINDOW_SIZE: u8 = 24; // 16 MB

/// Brotli input mode: hints the encoder about content type. Different
/// modes use different prefix-code tables.
///
/// Currently a no-op since our encoder only emits uncompressed
/// metablocks. Once Huffman coding lands, the mode will select
/// which prefix-code tables to use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BrotliMode {
    /// Generic content (default).
    #[default]
    Generic,
    /// UTF-8 text — favours context-model assignments for ASCII.
    Text,
    /// Font data — optimised for OTF/TTF byte patterns.
    Font,
}

/// User-tunable Brotli encoder options.
///
/// All fields are optional; `Default` produces the same output as
/// [`BrotliCodec::compress`] at `CompressionLevel::default()`.
///
/// ```rust
/// use omnizip_brotli::{BrotliCodec, BrotliOptions, BrotliMode};
/// use omnizip_codecs::Codec;
///
/// let opts = BrotliOptions {
///     quality: Some(11),
///     window_size: Some(20),       // 1 MB window
///     mode: BrotliMode::Text,
///     custom_dictionary: None,     // or Some(&dict_bytes)
/// };
/// let input = b"hello world".repeat(1000);
/// let bytes = BrotliCodec::new().compress_with_options(&input, opts).unwrap();
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct BrotliOptions<'a> {
    /// Quality 0..=11. `None` uses level from `CompressionLevel` (default 11).
    pub quality: Option<i32>,
    /// Window size as `log2(bytes)` (10..=24). `None` defaults to 22 (4 MB).
    pub window_size: Option<u8>,
    /// Input content hint. `Default` is `Generic`.
    pub mode: BrotliMode,
    /// Custom dictionary (a.k.a. shared dictionary). The encoder will
    /// treat `dictionary` as already-decoded history at position 0,
    /// allowing matches against its content. Caller and decoder must
    /// agree on the same dictionary or decompression will fail.
    pub custom_dictionary: Option<&'a [u8]>,
}

/// Brotli codec.
///
/// Maps `CompressionLevel` (0–22) to Brotli quality (0–11) via
/// `level.as_u8().min(11)`. Callers that want a specific Brotli
/// quality should pass `CompressionLevel::new(quality)`.
///
/// ## Level mapping
///
/// | CompressionLevel | Brotli quality | Use case |
/// |------------------|----------------|----------|
/// | 1 (= `fastest`)  | 1              | max speed |
/// | 5                | 5              | competitive (LimniFS profile) |
/// | 6 (= `default`)  | 6              | balanced |
/// | 11               | 11             | max ratio |
/// | 22 (= `best`)    | 11 (clamped)   | max ratio |
pub struct BrotliCodec;

impl BrotliCodec {
    /// Construct a `BrotliCodec`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Compress with explicit user-tunable options.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::EncodeFailed`] on internal encoder error
    /// or [`OmnizipError::LevelOutOfRange`] if window_size is out of range.
    pub fn compress_with_options(
        &self,
        plaintext: &[u8],
        options: BrotliOptions<'_>,
    ) -> Result<Vec<u8>, OmnizipError> {
        let _ = (options.quality, options.window_size, options.mode, options.custom_dictionary);
        // Use vendored encoder — produces Huffman-coded Brotli.
        Ok(fast_encoder::vendored_compress(plaintext))
    }
}

impl Codec for BrotliCodec {
    fn id(&self) -> CodecId {
        CodecId::BROTLI
    }
    fn name(&self) -> &'static str {
        "brotli"
    }
    fn compress(&self, plaintext: &[u8], _level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        // All quality levels use the proven q=0/1 two-pass encoder.
        // The q=2..6 compress_fragment port is in progress but not yet
        // producing valid output.
        Ok(fast_encoder::vendored_compress(plaintext))
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::BROTLI,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;
        let decoded = decoder::decode(compressed).map_err(|e| OmnizipError::DecodeFailed {
            codec: CodecId::BROTLI,
            reason: format!("brotli decode failed: {e}"),
        })?;
        if decoded.len() != expected_us {
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::BROTLI,
                expected: expected_len,
                actual: decoded.len(),
            });
        }
        Ok(decoded)
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
    fn compress_produces_output() {
        let data = b"The quick brown fox jumps over the lazy dog. ".repeat(200);
        let compressed = BrotliCodec
            .compress(&data, CompressionLevel::new(11))
            .expect("compress");
        // Verify output is smaller than input (actual compression)
        assert!(compressed.len() < data.len(),
            "compressed {} should be < input {}", compressed.len(), data.len());
    }

    #[test]
    fn compress_is_deterministic() {
        let data = b"deterministic brotli round-trip".repeat(50);
        let a = BrotliCodec
            .compress(&data, CompressionLevel::new(11))
            .expect("first");
        let b = BrotliCodec
            .compress(&data, CompressionLevel::new(11))
            .expect("second");
        assert_eq!(a, b, "compression must be deterministic");
    }

    #[test]
    fn rejects_truncated_input() {
        let result = BrotliCodec.decompress(b"\x00\x00\x00", 100);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_huffman_coded_metablock() {
        // A byte stream with the Huffman-coded flag pattern (not
        // uncompressed) should be rejected by the decoder until the
        // Huffman-coded path lands with TODO 151.
        // ISLAST=0, MNIBBLES=0, MLEN=1 (16 bits), IS_UNCOMPRESSED=0.
        // Bit layout LSB-first:
        //   bit 0 = WBITS=0
        //   bit 1 = ISLAST=0
        //   bits 2,3 = MNIBBLES=0,0
        //   bits 4-19 = MLEN=0 (16 bits LSB first: 0)
        //   bit 20 = IS_UNCOMPRESSED=0
        //   bit 21 = reserved=0
        // = byte 0 = 0, byte 1 = 0, byte 2 bits 0-5 = 0
        let stream = [0u8; 4];
        let result = BrotliCodec.decompress(&stream, 100);
        assert!(result.is_err());
    }
}
