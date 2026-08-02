//! Minimal ZPAQ-like container format.
//!
//! ```text
//! Header (11 bytes):
//!   magic:              b"ZPAQ\0"     (5 bytes)
//!   version:            u8 = 1
//!   config_id:          u8 = 1        (identifies the order-2 model config)
//!   uncompressed_size:  u32 LE
//! Body:
//!   arithmetic-coded bitstream
//! ```
//!
//! The container is intentionally minimal: no per-block metadata, no
//! journaling. Phase 2+ will extend it with multi-block segmentation and
//! a model description section.

#![forbid(unsafe_code)]

use crate::arithmetic::{ArithmeticDecoder, ArithmeticEncoder};
use crate::model::Order2Model;

/// Magic bytes identifying an omnizip-zpaq container.
pub const MAGIC: &[u8; 5] = b"ZPAQ\0";

/// Current container version.
pub const VERSION: u8 = 1;

/// Configuration identifier for the Phase 1 order-2 model.
pub const CONFIG_ID_ORDER2: u8 = 1;

/// Header length in bytes.
pub const HEADER_LEN: usize = 11;

/// Errors returned by the container layer.
#[derive(Debug)]
pub enum ContainerError {
    /// Magic bytes do not match.
    BadMagic,
    /// Unsupported container version.
    UnsupportedVersion(u8),
    /// Unknown model configuration id.
    UnknownConfig(u8),
    /// Header is shorter than [`HEADER_LEN`].
    TruncatedHeader,
    /// `uncompressed_size` exceeds `usize`.
    SizeOverflow,
}

impl std::fmt::Display for ContainerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "bad magic bytes"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported version {v}"),
            Self::UnknownConfig(c) => write!(f, "unknown config id {c}"),
            Self::TruncatedHeader => write!(f, "truncated header"),
            Self::SizeOverflow => write!(f, "uncompressed_size exceeds usize"),
        }
    }
}

impl std::error::Error for ContainerError {}

/// Wrap an arithmetic-coded payload with the container header.
///
/// `payload` is the raw encoder output stream. The function does not
/// re-encode the body — it prefixes the header.
#[must_use]
pub fn wrap(payload: &[u8], uncompressed_size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(CONFIG_ID_ORDER2);
    out.extend_from_slice(&uncompressed_size.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Decode a wrapped container, returning `(uncompressed_size, body_slice)`.
///
/// # Errors
///
/// Returns [`ContainerError`] if the header is malformed.
pub fn unwrap(data: &[u8]) -> Result<(u32, &[u8]), ContainerError> {
    if data.len() < HEADER_LEN {
        return Err(ContainerError::TruncatedHeader);
    }
    if &data[0..5] != MAGIC {
        return Err(ContainerError::BadMagic);
    }
    let version = data[5];
    if version != VERSION {
        return Err(ContainerError::UnsupportedVersion(version));
    }
    let config = data[6];
    if config != CONFIG_ID_ORDER2 {
        return Err(ContainerError::UnknownConfig(config));
    }
    let size = u32::from_le_bytes([data[7], data[8], data[9], data[10]]);
    Ok((size, &data[HEADER_LEN..]))
}

/// Convenience: compress a byte slice into a complete container.
///
/// Deterministic: identical inputs produce byte-identical outputs.
#[must_use]
pub fn compress_container(input: &[u8]) -> Vec<u8> {
    let mut enc = ArithmeticEncoder::new();
    let mut model = Order2Model::new();
    for &b in input {
        model.encode_byte(b, &mut enc);
    }
    let payload = enc.finish();
    let size = u32::try_from(input.len()).unwrap_or(u32::MAX);
    wrap(&payload, size)
}

/// Convenience: decompress a complete container.
///
/// # Errors
///
/// Returns [`ContainerError`] on malformed input.
pub fn decompress_container(data: &[u8]) -> Result<Vec<u8>, ContainerError> {
    let (size, body) = unwrap(data)?;
    let size_us = usize::try_from(size).map_err(|_| ContainerError::SizeOverflow)?;
    let mut dec = ArithmeticDecoder::new(body);
    let mut model = Order2Model::new();
    let mut out = Vec::with_capacity(size_us);
    for _ in 0..size_us {
        out.push(model.decode_byte(&mut dec));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty_container() {
        let compressed = compress_container(b"");
        let decompressed = decompress_container(&compressed).expect("decompress");
        assert_eq!(decompressed, b"");
        // Header is 11 bytes; the arithmetic encoder always flushes at
        // least one trailing byte to disambiguate the final sub-range.
        assert_eq!(compressed.len(), HEADER_LEN + 1);
    }

    #[test]
    fn round_trip_simple_text() {
        let input = b"hello world";
        let compressed = compress_container(input);
        let decompressed = decompress_container(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bad = compress_container(b"x");
        bad[0] = b'X';
        assert!(matches!(
            decompress_container(&bad),
            Err(ContainerError::BadMagic)
        ));
    }

    #[test]
    fn rejects_truncated_header() {
        assert!(matches!(
            decompress_container(b"ZP"),
            Err(ContainerError::TruncatedHeader)
        ));
    }

    #[test]
    fn rejects_unknown_version() {
        let mut bad = compress_container(b"x");
        bad[5] = 99;
        assert!(matches!(
            decompress_container(&bad),
            Err(ContainerError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn rejects_unknown_config() {
        let mut bad = compress_container(b"x");
        bad[6] = 99;
        assert!(matches!(
            decompress_container(&bad),
            Err(ContainerError::UnknownConfig(99))
        ));
    }

    #[test]
    fn header_layout_matches_spec() {
        let compressed = compress_container(b"AB");
        assert_eq!(&compressed[0..5], MAGIC);
        assert_eq!(compressed[5], VERSION);
        assert_eq!(compressed[6], CONFIG_ID_ORDER2);
        // size = 2 little-endian
        assert_eq!(&compressed[7..11], &[2, 0, 0, 0]);
    }
}
