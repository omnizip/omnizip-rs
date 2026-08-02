//! CRC32 (IEEE 802.3 / `bzip2` polynomial).
//!
//! The Ruby reference uses `Omnizip::Checksums::Crc32` which is the standard
//! zlib-compatible CRC32 (polynomial `0xEDB88320`, init `0xFFFFFFFF`, final
//! XOR `0xFFFFFFFF`, reflected). This module provides a table-free
//! implementation matching that exactly.

const POLY: u32 = 0xEDB8_8320;

/// Compute the CRC32 of `data`.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ POLY
            } else {
                crc >> 1
            };
        }
    }
    crc ^ 0xFFFF_FFFF
}

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
