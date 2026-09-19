//! CRC-64 family: XZ (reflected, ECMA-182 polynomial) and plain
//! ECMA-182 (non-reflected).
//!
//! Check values (the `123456789` convention, verified against the
//! CRC catalogue): CRC-64/XZ `0x995DC9BBDF1939FA`,
//! CRC-64/ECMA-182 `0x6C40DF5F0B497347`.

#![allow(clippy::cast_possible_truncation, clippy::cast_lossless)]

use std::sync::OnceLock;

use super::{Checksum, Digest};

/// CRC-64/XZ — `xz --check=crc64`: ECMA-182 polynomial in
/// reflected form, init and final XOR of all ones.
const POLY_REFLECTED: u64 = 0xC96C_5795_D787_0F42;

/// CRC-64/ECMA-182 — MSB-first, init 0, no final XOR.
const POLY_ECMA: u64 = 0x42F0_E1EB_A9EA_3693;

fn reflected_table() -> &'static [u64; 256] {
    static T: OnceLock<[u64; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u64; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u64;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ POLY_REFLECTED
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        t
    })
}

fn ecma_table() -> &'static [u64; 256] {
    static T: OnceLock<[u64; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u64; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = (i as u64) << 56;
            for _ in 0..8 {
                c = if c & 1 << 63 != 0 {
                    (c << 1) ^ POLY_ECMA
                } else {
                    c << 1
                };
            }
            *slot = c;
        }
        t
    })
}

/// CRC-64/XZ (what the xz container calls CRC64).
pub struct Crc64Xz {
    raw: u64,
}

impl Crc64Xz {
    #[must_use]
    pub fn new() -> Self {
        Self { raw: u64::MAX }
    }

    /// Factory for [`super::ChecksumRegistry::register`].
    pub fn boxed() -> Box<dyn Checksum> {
        Box::new(Self::new())
    }
}

impl Default for Crc64Xz {
    fn default() -> Self {
        Self::new()
    }
}

impl Checksum for Crc64Xz {
    fn update(&mut self, data: &[u8]) {
        let t = reflected_table();
        for &b in data {
            let idx = ((self.raw ^ u64::from(b)) & 0xFF) as usize;
            self.raw = (self.raw >> 8) ^ t[idx];
        }
    }
    fn finish(&self) -> Digest {
        Digest::U64(!self.raw)
    }
}

/// CRC-64/ECMA-182 (non-reflected).
pub struct Crc64Ecma {
    state: u64,
}

impl Crc64Ecma {
    #[must_use]
    pub fn new() -> Self {
        Self { state: 0 }
    }

    /// Factory for [`super::ChecksumRegistry::register`].
    pub fn boxed() -> Box<dyn Checksum> {
        Box::new(Self::new())
    }
}

impl Default for Crc64Ecma {
    fn default() -> Self {
        Self::new()
    }
}

impl Checksum for Crc64Ecma {
    fn update(&mut self, data: &[u8]) {
        let t = ecma_table();
        for &b in data {
            self.state = (self.state << 8) ^ t[(((self.state >> 56) as u8) ^ b) as usize];
        }
    }
    fn finish(&self) -> Digest {
        Digest::U64(self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_check_values() {
        let mut xz = Crc64Xz::new();
        xz.update(b"123456789");
        assert_eq!(xz.finish(), Digest::U64(0x995D_C9BB_DF19_39FA));

        let mut ecma = Crc64Ecma::new();
        ecma.update(b"123456789");
        assert_eq!(ecma.finish(), Digest::U64(0x6C40_DF5F_0B49_7347));
    }

    #[test]
    fn empty_inputs() {
        assert_eq!(Crc64Xz::new().finish(), Digest::U64(!u64::MAX));
        assert_eq!(Crc64Ecma::new().finish(), Digest::U64(0));
    }
}
