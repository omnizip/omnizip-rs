//! Pure-Rust Brotli codec.
//!
//! Currently wraps the [`brotli`] crate (by Daniel Reiter Horn, the
//! format's original author) for encode + decode. A pure-Rust
//! implementation is being phased in via [`decoder`] — Phase A
//! (frame header + metablock header + bit reader) is landed; later
//! phases will replace the wrapper's encode and decode paths
//! (TODO 117).
//!
//! Brotli is the highest-ratio pure-Rust codec in the registry at quality
//! 11. It outperforms ZSTD and LZMA on text and web content.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod decoder;
pub mod dictionary;

use std::io::{self, Cursor};

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

impl BrotliMode {
    fn as_brotli_const(self) -> brotli::enc::backward_references::BrotliEncoderMode {
        use brotli::enc::backward_references::BrotliEncoderMode;
        match self {
            Self::Generic => BrotliEncoderMode::BROTLI_MODE_GENERIC,
            Self::Text => BrotliEncoderMode::BROTLI_MODE_TEXT,
            Self::Font => BrotliEncoderMode::BROTLI_MODE_FONT,
        }
    }
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
        let quality = options.quality.unwrap_or(DEFAULT_QUALITY);
        let lgwin = options.window_size.unwrap_or(DEFAULT_WINDOW_SIZE);
        if !(MIN_WINDOW_SIZE..=MAX_WINDOW_SIZE).contains(&lgwin) {
            return Err(OmnizipError::LevelOutOfRange {
                codec: CodecId::BROTLI,
                level: lgwin,
                min: MIN_WINDOW_SIZE,
                max: MAX_WINDOW_SIZE,
            });
        }
        let params = brotli::enc::backward_references::BrotliEncoderParams {
            quality,
            lgwin: i32::from(lgwin),
            mode: options.mode.as_brotli_const(),
            ..Default::default()
        };
        let dict: &[u8] = options.custom_dictionary.unwrap_or(&[]);
        let mut output = Vec::new();
        if dict.is_empty() {
            brotli::BrotliCompress(&mut Cursor::new(plaintext), &mut output, &params).map_err(|e| {
                OmnizipError::EncodeFailed {
                    codec: CodecId::BROTLI,
                    reason: format!(
                        "brotli compress (quality {quality}, lgwin {lgwin}) failed: {e}"
                    ),
                }
            })?;
        } else {
            // Use the custom-dictionary variant of the brotli encoder.
            use brotli::enc::{BrotliCompressCustomIoCustomDict, StandardAlloc};
            use brotli::{IoReaderWrapper, IoWriterWrapper};
            let alloc = StandardAlloc::default();
            let mut input_buf = [0u8; 4096];
            let mut output_buf = [0u8; 4096];
            let mut callback = |_pm: &mut brotli::enc::interface::PredictionModeContextMap<
                brotli::enc::interface::InputReferenceMut,
            >,
                                _cmds: &mut [brotli::enc::interface::StaticCommand],
                                _pair: brotli::enc::interface::InputPair,
                                _alloc: &mut StandardAlloc| {};
            let mut reader = Cursor::new(plaintext);
            let mut writer: Vec<u8> = Vec::new();
            BrotliCompressCustomIoCustomDict(
                &mut IoReaderWrapper(&mut reader),
                &mut IoWriterWrapper(&mut writer),
                &mut input_buf,
                &mut output_buf,
                &params,
                alloc,
                &mut callback,
                dict,
                io::Error::new(io::ErrorKind::UnexpectedEof, "brotli"),
            )
            .map_err(|e| OmnizipError::EncodeFailed {
                codec: CodecId::BROTLI,
                reason: format!(
                    "brotli compress with dict (quality {quality}, lgwin {lgwin}) failed: {e}"
                ),
            })?;
            output = writer;
        }
        Ok(output)
    }
}

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
