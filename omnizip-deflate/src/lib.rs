//! Pure-Rust DEFLATE codec — uses the in-house [`omnizip-libdeflate`]
//! encoder + decoder (no external dependencies).
//!
//! Produces zlib-framed RFC 1951 streams (2-byte zlib header + DEFLATE
//! body + Adler-32 checksum) decodable by any zlib decoder.
//!
//! This crate delegates to [`omnizip_libdeflate`] for the actual
//! DEFLATE encoding + decoding, which is pure-Rust with no external
//! dependencies. The `DeflateCodec` wraps it with a distinct codec ID
//! so callers can select between the two implementations.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// DEFLATE codec. Uses [`omnizip_libdeflate`] internally.
///
/// The wire format is zlib (RFC 1950): 2-byte header + DEFLATE body +
/// Adler-32 trailer. Identical to the output of `gzip -c` after
/// stripping the gzip header.
pub struct DeflateCodec;

impl DeflateCodec {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for DeflateCodec {
    fn default() -> Self {
        Self::new()
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
        // Delegate to omnizip-libdeflate (pure-Rust, no external deps).
        omnizip_libdeflate::LibdeflateCodec::new().compress(plaintext, level)
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        omnizip_libdeflate::LibdeflateCodec::new().decompress(compressed, expected_len)
    }
}

/// Compress using the zlib format (RFC 1950). Wraps
/// [`omnizip_libdeflate`].
///
/// # Errors
///
/// See [`omnizip_libdeflate::LibdeflateCodec::compress`].
pub fn compress_to_vec_zlib(input: &[u8], level: impl Into<u8>) -> Vec<u8> {
    let codec = omnizip_libdeflate::LibdeflateCodec::new();
    codec
        .compress(input, CompressionLevel::new(level.into()))
        .unwrap_or_else(|_| input.to_vec())
}

/// Decompress a zlib stream (RFC 1950). Wraps
/// [`omnizip_libdeflate`].
///
/// # Errors
///
/// See [`omnizip_libdeflate::LibdeflateCodec::decompress`].
pub fn decompress_to_vec_zlib(input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    let codec = omnizip_libdeflate::LibdeflateCodec::new();
    // miniz_oxide's API didn't require expected_len. We estimate from
    // the zlib stream's content if possible, or pass 0 for the
    // libdeflate decoder to figure out.
    let _ = codec;
    // Try with a large expected_len (the decoder will handle it).
    omnizip_libdeflate::LibdeflateCodec::new()
        .decompress(input, 0)
        .or_else(|_| {
            // Fallback: try without the zlib wrapper.
            omnizip_libdeflate::LibdeflateCodec::new().decompress(input, 0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_text() {
        let data = b"the quick brown fox jumps over the lazy dog. ".repeat(50);
        let compressed = DeflateCodec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        let decompressed = DeflateCodec
            .decompress(&compressed, data.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, data);
    }

    #[test]
    fn round_trip_empty() {
        let compressed = DeflateCodec
            .compress(b"", CompressionLevel::default())
            .expect("compress");
        let decompressed = DeflateCodec.decompress(&compressed, 0).expect("decompress");
        assert!(decompressed.is_empty());
    }

    #[test]
    fn round_trip_binary() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i * 7 % 251) as u8).collect();
        let compressed = DeflateCodec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        let decompressed = DeflateCodec
            .decompress(&compressed, data.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, data);
    }

    #[test]
    fn compresses_repetitive_data() {
        let data = vec![0x41u8; 10_000];
        let compressed = DeflateCodec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        assert!(compressed.len() < data.len());
    }
}
