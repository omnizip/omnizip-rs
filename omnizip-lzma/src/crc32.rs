//! CRC32 (IEEE 802.3 polynomial `0xEDB88320`) — used by XZ stream
//! headers, block headers, and the lzip trailing checksum.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use std::sync::OnceLock;

static TABLE: OnceLock<[u32; 256]> = OnceLock::new();

#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let table = TABLE.get_or_init(build_table);
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        let idx = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = (crc >> 8) ^ table[idx];
    }
    crc ^ 0xFFFF_FFFF
}

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

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
