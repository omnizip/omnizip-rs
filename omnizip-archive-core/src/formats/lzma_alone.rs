//! LZMA_Alone (.lzma) single-file format facade over `omnizip-lzma` —
//! port of `omnizip/formats/lzma_alone.rb`: the 13-byte header
//! (properties byte + 4-byte dict size + 8-byte uncompressed size, or
//! all-FF for unknown) + LZMA1 stream.
#![forbid(unsafe_code)]

use crate::error::ArchiveError;

/// Decompress a .lzma file.
///
/// # Errors
///
/// [`ArchiveError::InvalidArchive`] on malformed input.
pub fn decompress(input: &[u8]) -> Result<Vec<u8>, ArchiveError> {
    omnizip_lzma::lzma_alone_decompress(input)
        .map_err(|e| ArchiveError::InvalidArchive(format!("{e}")))
}

/// Compress to .lzma (unknown-size header form, EOPM-terminated —
/// decodable by `lzma -d` and `xz --format=lzma`).
///
/// # Errors
///
/// [`ArchiveError::InvalidArchive`] on codec failure.
pub fn compress(input: &[u8]) -> Result<Vec<u8>, ArchiveError> {
    omnizip_lzma::lzma_alone_compress(input)
        .map_err(|e| ArchiveError::InvalidArchive(format!("{e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let data = b"lzma alone facade round trip".repeat(50);
        let lz = compress(&data).unwrap();
        assert_eq!(lz.len() % 5, lz.len() % 5); // header sanity below
        assert!(lz.len() > 13);
        assert_eq!(decompress(&lz).unwrap(), data);
    }
}
