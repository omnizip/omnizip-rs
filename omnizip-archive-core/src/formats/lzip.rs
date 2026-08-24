//! LZIP (.lz) single-file format facade over `omnizip-lzma` — port of
//! `omnizip/formats/lzip.rb`. Lzma2-free container: magic "LZIP" +
//! version + dict-size byte, LZMA1 stream, CRC32 + ISIZE(8) trailer
//! (the encoder side of the framing; the decoder ships in
//! omnizip-lzma).
#![forbid(unsafe_code)]

use crate::error::ArchiveError;

/// LZIP magic.
pub const LZIP_MAGIC: [u8; 4] = *b"LZIP";
/// Current lzip version byte (v1).
pub const LZIP_VERSION: u8 = 1;

/// One-member lzip metadata.
#[derive(Clone, Debug)]
pub struct LzipOptions {
    /// Dictionary size as the base-2 log encoded byte (0 = default
    /// from the codec).
    pub dict_size: Option<u8>,
}

impl Default for LzipOptions {
    fn default() -> Self {
        Self { dict_size: None }
    }
}

/// Compress `input` into a complete lzip member.
///
/// # Errors
///
/// [`ArchiveError::InvalidArchive`] on codec failure.
pub fn compress(input: &[u8], _options: &LzipOptions) -> Result<Vec<u8>, ArchiveError> {
    // omnizip-lzma's lzip_compress emits the full framing (magic,
    // version, dict byte, LZMA1 data, CRC32 + 64-bit size trailer).
    omnizip_lzma::lzip_compress(input).map_err(|e| ArchiveError::InvalidArchive(format!("{e}")))
}

/// Decompress a (possibly multi-member) lzip file.
///
/// # Errors
///
/// [`ArchiveError::InvalidArchive`] on malformed input.
pub fn decompress(input: &[u8]) -> Result<Vec<u8>, ArchiveError> {
    omnizip_lzma::lzip_decompress(input).map_err(|e| ArchiveError::InvalidArchive(format!("{e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let data = b"lzip facade round trip".repeat(50);
        let lz = compress(&data, &LzipOptions::default()).unwrap();
        assert_eq!(&lz[..4], &LZIP_MAGIC);
        assert_eq!(decompress(&lz).unwrap(), data);
    }
}
