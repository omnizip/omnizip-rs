//! CRC-32 (IEEE 802.3 / zlib polynomial `0xEDB88320`).
//!
//! Delegates to the shared slice-by-8 implementation in
//! `omnizip_codecs::checksum`. See `TODO.complete/82-simd-crc32-xxhash.md`
//! and `TODO.complete/94-dry-crc32-migration.md`.

#![forbid(unsafe_code)]

pub use omnizip_codecs::checksum::crc32_iso_hdlc as crc32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // Standard CRC32 test vectors (zlib/python `zlib.crc32` & Ruby
        // `Zlib.crc32` agree).
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }
}
