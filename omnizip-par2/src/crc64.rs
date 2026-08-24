//! CRC-64/XZ (poly 0x42F0E1EBA9EA3693, reflected 0xC96C5795D7870F42,
//! init/xorout all-ones) — the PAR2 slice-check checksum.
#![forbid(unsafe_code)]

fn table() -> &'static [u64; 256] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[u64; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [0u64; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut crc = i as u64;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xC96C_5795_D787_0F42
                } else {
                    crc >> 1
                };
            }
            *slot = crc;
        }
        t
    })
}

/// CRC-64/XZ of `data`.
#[must_use]
pub fn crc64(data: &[u8]) -> u64 {
    let t = table();
    let mut crc = u64::MAX;
    for &b in data {
        crc = t[((crc ^ u64::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(crc64(b""), 0);
        // CRC-64/XZ("123456789") = 0x995DC9BBDF1939FA
        assert_eq!(crc64(b"123456789"), 0x995D_C9BB_DF19_39FA);
        // CRC-64/XZ("a")
        assert_eq!(crc64(b"a"), 0x3302_8477_2E65_2B05);
    }
}
