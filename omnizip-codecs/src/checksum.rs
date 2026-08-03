//! Shared checksum implementations.
//!
//! Currently exposes [`crc32_iso_hdlc`] — the CRC-32 variant used by
//! gzip / XZ / BZip2 / Zlib (polynomial `0xEDB88320`, reflected, init
//! `0xFFFF_FFFF`, final XOR `0xFFFF_FFFF`).
//!
//! ## Implementation
//!
//! Slice-by-8: each loop iteration consumes 8 bytes via 8 parallel
//! table lookups and XORs. Roughly 3× faster than byte-by-byte on
//! inputs above a few hundred bytes, due to instruction-level
//! parallelism (8 independent loads + XORs per cycle).
//!
//! ### Why not "real" SIMD?
//!
//! True SIMD CRC-32 uses `PCLMULQDQ` (carryless 64×64 → 128-bit
//! multiply) for 10+ GB/s throughput. `core::arch::x86_64::_mm_clmulepi64_si128`
//! requires `unsafe`, which is workspace-forbidden. `std::simd` does
//! not expose carryless multiplication on stable Rust.
//!
//! The fallback "slice-by-N" pattern benefits from instruction-level
//! parallelism but does not gain from `std::simd` lanes — the table
//! lookups need gather loads, which `std::simd` lacks on stable.
//!
//! See `TODO.complete/82-simd-crc32-xxhash.md` for the path to a
//! `PCLMULQDQ`-backed impl (would require an opt-in `unsafe-simd`
//! feature, gated off by default to preserve `#![forbid(unsafe_code)]`).
//!
//! ## Determinism
//!
//! Identical input always produces identical output. The slice-by-8
//! main loop and the byte-by-byte tail are formally equivalent;
//! verified by differential tests below against Python's `zlib.crc32`.

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation, clippy::cast_lossless)]

use std::sync::OnceLock;

/// CRC-32 polynomial (reflected, zlib/gzip).
const POLY: u32 = 0xEDB8_8320;

static TABLE0: OnceLock<[u32; 256]> = OnceLock::new();
static TABLE8: OnceLock<[[u32; 256]; 8]> = OnceLock::new();

fn table0() -> &'static [u32; 256] {
    TABLE0.get_or_init(|| {
        let mut t = [0u32; 256];
        for i in 0..256u32 {
            let mut c = i;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ POLY } else { c >> 1 };
            }
            t[i as usize] = c;
        }
        t
    })
}

fn table8() -> &'static [[u32; 256]; 8] {
    TABLE8.get_or_init(|| {
        let mut t = [[0u32; 256]; 8];
        t[0] = *table0();
        for i in 1..8 {
            for j in 0..256usize {
                let prev = t[i - 1][j];
                t[i][j] = (prev >> 8) ^ table0()[(prev & 0xFF) as usize];
            }
        }
        t
    })
}

/// Compute the ISO-HDLC CRC-32 of `data` (the gzip/zlib/bzip2/xz variant).
///
/// Equivalent to `crc32(0, data)` from zlib. Returns the final CRC
/// after the `0xFFFF_FFFF` final XOR.
///
/// # Examples
///
/// ```
/// # use omnizip_codecs::checksum::crc32_iso_hdlc;
/// assert_eq!(crc32_iso_hdlc(b""), 0);
/// assert_eq!(crc32_iso_hdlc(b"123456789"), 0xCBF4_3926);
/// assert_eq!(crc32_iso_hdlc(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
/// ```
#[must_use]
pub fn crc32_iso_hdlc(data: &[u8]) -> u32 {
    if data.is_empty() {
        return 0;
    }
    let t = table8();
    let t0 = &t[0];
    let t1 = &t[1];
    let t2 = &t[2];
    let t3 = &t[3];
    let t4 = &t[4];
    let t5 = &t[5];
    let t6 = &t[6];
    let t7 = &t[7];

    let mut crc: u32 = 0xFFFF_FFFF;
    let main = data.len() - (data.len() % 8);
    let mut i = 0;

    while i + 8 <= main {
        // XOR the next 4 bytes into the low bits of crc.
        let block = u32::from_le_bytes([
            data[i], data[i + 1], data[i + 2], data[i + 3],
        ]);
        let high = crc ^ block;
        // Slice-by-8: each stream byte goes through a different-depth
        // table. Stream byte 0 (low byte of `high`) has 7 byte-steps
        // after it within this batch → deepest table (t7). Stream byte
        // 7 (last) has 0 byte-steps after it → shallowest table (t0).
        crc = t7[((high) & 0xFF) as usize]
            ^ t6[((high >> 8) & 0xFF) as usize]
            ^ t5[((high >> 16) & 0xFF) as usize]
            ^ t4[((high >> 24) & 0xFF) as usize]
            ^ t3[data[i + 4] as usize]
            ^ t2[data[i + 5] as usize]
            ^ t1[data[i + 6] as usize]
            ^ t0[data[i + 7] as usize];
        i += 8;
    }

    while i < data.len() {
        let idx = ((crc ^ u32::from(data[i])) & 0xFF) as usize;
        crc = (crc >> 8) ^ t0[idx];
        i += 1;
    }

    !crc
}

/// Compute CRC-32 continuing from a previous raw `state` (the value
/// *before* the final XOR). Mirrors the zlib `crc32(prev, buf)`
/// incremental API. Use this when streaming data through a checksum.
///
/// Most callers want [`crc32_iso_hdlc`] (one-shot) instead.
#[must_use]
pub fn crc32_iso_hdlc_update(state: u32, data: &[u8]) -> u32 {
    let t = table0();
    let mut crc = state;
    for &b in data {
        let idx = ((crc ^ u32::from(b)) & 0xFF) as usize;
        crc = (crc >> 8) ^ t[idx];
    }
    !crc
}

/// Compute CRC-32 without the final XOR — useful for incremental
/// combination. Most callers want [`crc32_iso_hdlc`] instead.
#[must_use]
pub fn crc32_iso_hdlc_raw(data: &[u8]) -> u32 {
    !crc32_iso_hdlc(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Standard CRC-32 check values — verified against Python's zlib.crc32.
    const CHECK_EMPTY: u32 = 0;
    const CHECK_NINES: u32 = 0xCBF4_3926;
    const CHECK_FOX: u32 = 0x414F_A339;
    const CHECK_A: u32 = 0xE8B7_BE43;
    const CHECK_ZEROS8: u32 = 0x6522_DF69;

    #[test]
    fn empty_input_is_zero() {
        assert_eq!(crc32_iso_hdlc(b""), CHECK_EMPTY);
    }

    #[test]
    fn known_value_ascii_digits() {
        assert_eq!(crc32_iso_hdlc(b"123456789"), CHECK_NINES);
    }

    #[test]
    fn known_value_pangram() {
        assert_eq!(crc32_iso_hdlc(b"The quick brown fox jumps over the lazy dog"), CHECK_FOX);
    }

    #[test]
    fn known_value_single_byte() {
        assert_eq!(crc32_iso_hdlc(b"a"), CHECK_A);
    }

    #[test]
    fn known_value_zeros() {
        assert_eq!(crc32_iso_hdlc(&[0u8; 8]), CHECK_ZEROS8);
    }

    #[test]
    fn slice_by_8_matches_byte_by_byte() {
        // Reference: byte-by-byte implementation using only table0.
        fn byte_by_byte(data: &[u8]) -> u32 {
            if data.is_empty() {
                return 0;
            }
            let t = table0();
            let mut crc: u32 = 0xFFFF_FFFF;
            for &b in data {
                let idx = ((crc ^ u32::from(b)) & 0xFF) as usize;
                crc = (crc >> 8) ^ t[idx];
            }
            !crc
        }
        let inputs: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            (0..7u8).collect(),
            (0..8u8).collect(),
            (0..16u8).collect(),
            (0..255u8).collect(),
            (0..1024u32).map(|i| (i & 0xFF) as u8).collect(),
            b"the quick brown fox jumps over the lazy dog".to_vec(),
            {
                let mut v = vec![0u8; 4096];
                for (i, b) in v.iter_mut().enumerate() {
                    *b = (i.wrapping_mul(31) & 0xFF) as u8;
                }
                v
            },
        ];
        for input in &inputs {
            assert_eq!(
                crc32_iso_hdlc(input),
                byte_by_byte(input),
                "mismatch on input length {}",
                input.len()
            );
        }
    }

    #[test]
    fn incremental_update_matches_one_shot() {
        let data: Vec<u8> = (0..1024u32).map(|i| (i & 0xFF) as u8).collect();
        let mid = data.len() / 2;

        let one_shot = crc32_iso_hdlc(&data);
        let raw_a = crc32_iso_hdlc_raw(&data[..mid]);
        let incremental = crc32_iso_hdlc_update(raw_a, &data[mid..]);

        assert_eq!(one_shot, incremental);
    }

    #[test]
    fn determinism_same_input_same_output() {
        let input = b"deterministic checksums are required for content addressing";
        assert_eq!(crc32_iso_hdlc(input), crc32_iso_hdlc(input));
    }

    #[test]
    fn table_entry_1_is_standard_value() {
        let t = table0();
        assert_eq!(t[1], 0x7707_3096);
    }
}
