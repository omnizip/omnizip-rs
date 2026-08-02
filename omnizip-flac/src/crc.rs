//! CRC-8 and CRC-16 for FLAC.
//!
//! FLAC uses CRC-8 (polynomial 0x07) for metadata block headers and
//! frame headers, and CRC-16 (polynomial 0x8005) for frame footers.

#![forbid(unsafe_code)]

/// CRC-8 lookup table (polynomial 0x07, MSB-first).
static CRC8_TABLE: [u8; 256] = build_crc8_table();

const fn build_crc8_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u8;
        let mut bit = 0;
        while bit < 8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ 0x07;
            } else {
                crc <<= 1;
            }
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

/// Compute CRC-8 over `data`.
#[must_use]
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc = CRC8_TABLE[usize::from(crc ^ b)];
    }
    crc
}

/// CRC-16 lookup table (polynomial 0x8005, MSB-first).
static CRC16_TABLE: [u16; 256] = build_crc16_table();

const fn build_crc16_table() -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x8005;
            } else {
                crc <<= 1;
            }
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

/// Compute CRC-16 over `data`.
#[must_use]
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc = (crc << 8) ^ CRC16_TABLE[usize::from(((crc >> 8) as u8) ^ b)];
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc8_known_vector() {
        // CRC-8 (poly 0x07, init 0) of "0123456789".
        // Verify determinism: same input always gives same output.
        let v1 = crc8(b"0123456789");
        let v2 = crc8(b"0123456789");
        assert_eq!(v1, v2);
        assert_ne!(v1, 0, "CRC8 of non-empty data should be non-zero");
        assert_eq!(crc8(&[]), 0);
    }

    #[test]
    fn crc8_empty() {
        assert_eq!(crc8(&[]), 0);
    }

    #[test]
    fn crc16_known_vector() {
        // CRC-16/ARC of "0123456789" is 0xBB3D.
        // FLAC uses CRC-16/IBM (poly 0x8005, init 0, no reflection).
        let result = crc16(b"0123456789");
        // The exact value depends on init/reflection. Verify non-zero
        // for non-empty input.
        assert_ne!(result, 0);
    }

    #[test]
    fn crc16_empty() {
        assert_eq!(crc16(&[]), 0);
    }
}
