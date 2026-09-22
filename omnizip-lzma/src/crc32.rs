//! CRC-32 (IEEE 802.3 polynomial `0xEDB88320`) — used by XZ stream
//! headers, block headers, and the lzip trailing checksum.
//!
//! Delegates to the shared slice-by-8 implementation in
//! `omnizip_codecs::checksum`. (task record in git history).
//! and (task record in git history).

#![forbid(unsafe_code)]

pub use omnizip_codecs::checksum::crc32_iso_hdlc as crc32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_zero_after_conditioning() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn known_vector_ascii_a() {
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
    }

    #[test]
    fn known_vector_hello() {
        assert_eq!(crc32(b"Hello"), 0xF7D1_8982);
    }

    #[test]
    fn deterministic_across_calls() {
        assert_eq!(crc32(b"test"), crc32(b"test"));
    }
}
