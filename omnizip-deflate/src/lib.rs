//! Pure-Rust DEFLATE codec — wraps [`miniz_oxide`] behind the
//! [`omnizip_codecs::Codec`] trait.
//!
//! Produces zlib-framed RFC 1951 streams (2-byte zlib header + DEFLATE
//! body + Adler-32 checksum) decodable by any zlib decoder.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// Output format wrapping the raw DEFLATE body.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeflateFormat {
    /// zlib framing (RFC 1950): 2-byte header + DEFLATE + Adler-32.
    /// Default; what `compress_to_vec_zlib` produces.
    #[default]
    Zlib,
    /// Raw DEFLATE (RFC 1951) — no header/trailer.
    Raw,
    /// gzip framing (RFC 1952): 10-byte header + DEFLATE + CRC-32 + size.
    Gzip,
}

/// DEFLATE compression strategy (matches zlib's `Z_*` constants).
///
/// Selectable via [`DeflateOptions::strategy`]. Different strategies
/// favour different match-finder heuristics; some inputs compress
/// better with `Filtered` (e.g. structured data with many short
/// matches) or `HuffmanOnly` (high-entropy input).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeflateStrategy {
    /// Standard LZ77 + Huffman. Default.
    #[default]
    Default,
    /// Only use matches ≥5 bytes. Better for structured data
    /// (e.g. tables, code with predictable structure).
    Filtered,
    /// Huffman-only — skip LZ77. Fastest on high-entropy input.
    HuffmanOnly,
    /// Run-length only — only matches at distance 1.
    Rle,
    /// Fixed Huffman codes only (no dynamic tables).
    Fixed,
}

impl DeflateStrategy {
    fn as_miniz_const(self) -> miniz_oxide::deflate::core::CompressionStrategy {
        use miniz_oxide::deflate::core::CompressionStrategy;
        match self {
            Self::Default => CompressionStrategy::Default,
            Self::Filtered => CompressionStrategy::Filtered,
            Self::HuffmanOnly => CompressionStrategy::HuffmanOnly,
            Self::Rle => CompressionStrategy::RLE,
            Self::Fixed => CompressionStrategy::Fixed,
        }
    }
}

/// User-tunable DEFLATE options.
///
/// ```rust
/// use omnizip_deflate::{DeflateCodec, DeflateFormat, DeflateOptions, DeflateStrategy};
/// use omnizip_codecs::Codec;
///
/// let opts = DeflateOptions {
///     level: 9,
///     format: DeflateFormat::Gzip,
///     strategy: DeflateStrategy::Filtered,
/// };
/// let input = b"hello".repeat(100);
/// let bytes = DeflateCodec.compress_with_options(&input, opts).unwrap();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct DeflateOptions {
    /// Compression level 0..=9.
    pub level: u8,
    /// Output framing format.
    pub format: DeflateFormat,
    /// Match-finder strategy.
    pub strategy: DeflateStrategy,
}

impl Default for DeflateOptions {
    fn default() -> Self {
        Self {
            level: 6,
            format: DeflateFormat::Zlib,
            strategy: DeflateStrategy::Default,
        }
    }
}

/// DEFLATE codec. Levels 0–9 map to `miniz_oxide` levels.
pub struct DeflateCodec;

impl DeflateCodec {
    /// Construct a `DeflateCodec`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Compress with explicit user-tunable options (level + format + strategy).
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::EncodeFailed`] on internal encoder error.
    pub fn compress_with_options(
        &self,
        plaintext: &[u8],
        options: DeflateOptions,
    ) -> Result<Vec<u8>, OmnizipError> {
        let body = compress_deflate_with_strategy(plaintext, options.level, options.strategy);
        Ok(match options.format {
            DeflateFormat::Raw => body,
            DeflateFormat::Zlib => wrap_zlib(plaintext, &body),
            DeflateFormat::Gzip => wrap_gzip(plaintext, &body),
        })
    }
}

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

/// Compress raw DEFLATE body using miniz_oxide's lower-level API to
/// expose the strategy parameter. Falls back to the public
/// `compress_to_vec` when strategy is `Default`.
fn compress_deflate_with_strategy(
    plaintext: &[u8],
    level: u8,
    strategy: DeflateStrategy,
) -> Vec<u8> {
    if strategy == DeflateStrategy::Default {
        // Fast path — use the public API (identical output, no callback overhead).
        let miniz_level = clamp_level(CompressionLevel::new(level));
        return miniz_oxide::deflate::compress_to_vec(plaintext, miniz_level);
    }

    use miniz_oxide::deflate::core::{
        compress_to_output, create_comp_flags_from_zip_params, CompressorOxide, TDEFLFlush,
        TDEFLStatus,
    };

    let level_i32 = i32::from(clamp_level(CompressionLevel::new(level)));
    let strategy_i32 = i32::from(strategy.as_miniz_const());
    let flags = create_comp_flags_from_zip_params(level_i32, 0, strategy_i32);
    let mut compressor = CompressorOxide::new(flags);
    let mut out = Vec::with_capacity(plaintext.len());
    let mut consumed = 0;
    let in_buf = plaintext;
    // Cap iterations to avoid pathological loops on misbehaving input.
    for _ in 0..1_000_000 {
        let mut chunk_out: Vec<u8> = Vec::with_capacity(64 * 1024);
        let (status, in_consumed) = compress_to_output(
            &mut compressor,
            &in_buf[consumed..],
            TDEFLFlush::Finish,
            |bytes: &[u8]| {
                chunk_out.extend_from_slice(bytes);
                true
            },
        );
        out.extend_from_slice(&chunk_out);
        consumed += in_consumed;
        if status == TDEFLStatus::Done {
            return out;
        }
        if status == TDEFLStatus::BadParam || status == TDEFLStatus::PutBufFailed {
            // Fallback to default strategy.
            return miniz_oxide::deflate::compress_to_vec(
                plaintext,
                clamp_level(CompressionLevel::new(level)),
            );
        }
        if in_consumed == 0 {
            // No progress in this iteration — bail to avoid infinite loop.
            return miniz_oxide::deflate::compress_to_vec(
                plaintext,
                clamp_level(CompressionLevel::new(level)),
            );
        }
        if consumed >= in_buf.len() {
            // All input consumed; one more Finish call to flush should
            // have produced Done. If not, break to avoid infinite loop.
            return out;
        }
    }
    out
}

/// Wrap a raw DEFLATE body in a zlib header/trailer.
fn wrap_zlib(plaintext: &[u8], deflate_body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(deflate_body.len() + 6);
    // zlib header: CMF=0x78 (deflate, 32 KB window), FLG=0x9C (default level).
    out.push(0x78);
    out.push(0x9C);
    out.extend_from_slice(deflate_body);
    // Adler-32 of the original plaintext, big-endian.
    let adler = adler32(plaintext);
    out.extend_from_slice(&adler.to_be_bytes());
    out
}

/// Wrap a raw DEFLATE body in a gzip header/trailer.
fn wrap_gzip(plaintext: &[u8], deflate_body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(deflate_body.len() + 18);
    // gzip header (RFC 1952): magic, method=deflate, no flags, mtime=0,
    // extra flags=0, OS=255 (unknown).
    out.extend_from_slice(&[0x1F, 0x8B, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF]);
    out.extend_from_slice(deflate_body);
    // CRC-32 of the original plaintext, little-endian.
    let crc = crc32(plaintext);
    out.extend_from_slice(&crc.to_le_bytes());
    // Original size mod 2^32, little-endian.
    let len = u32::try_from(plaintext.len()).unwrap_or(0);
    out.extend_from_slice(&len.to_le_bytes());
    out
}

/// Standard Adler-32 checksum (RFC 1950).
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + u32::from(byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

/// Standard CRC-32 (polynomial 0xEDB88320, same as zlib/gzip).
///
/// Delegates to the shared slice-by-8 implementation in
/// `omnizip_codecs::checksum`. See `TODO.complete/94-dry-crc32-migration.md`.
fn crc32(data: &[u8]) -> u32 {
    omnizip_codecs::checksum::crc32_iso_hdlc(data)
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
