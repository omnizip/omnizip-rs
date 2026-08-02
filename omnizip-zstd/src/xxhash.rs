//! `XXHash` — ZSTD frame checksum.
//!
//! Per the C reference (`zstd_decompress.c:1052`), ZSTD uses
//! **XXH64 truncated to 32 bits**, not XXH32 as RFC 8878 §4.2.4
//! ambiguously suggests.
//!
//! Verified against `~/src/external/zstd/lib/common/xxhash.h`.

#![forbid(unsafe_code)]

const PRIME32_1: u32 = 0x9E37_79B1;
const PRIME32_2: u32 = 0x85EB_CA77;
const PRIME32_3: u32 = 0xC2B2_AE3D;
const PRIME32_4: u32 = 0x27D4_EB2F;
const PRIME32_5: u32 = 0x1656_67B1;

const PRIME64_1: u64 = 0x9E37_79B1_85EB_CA87;
const PRIME64_2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const PRIME64_3: u64 = 0x1656_67B1_9E37_79F9;
const PRIME64_4: u64 = 0x85EB_CA77_C2B2_AE63;
const PRIME64_5: u64 = 0x27D4_EB2F_1656_67C5;

#[inline]
fn rotl(x: u32, r: u32) -> u32 {
    x.rotate_left(r)
}

#[inline]
fn rotl64(x: u64, r: u32) -> u64 {
    x.rotate_left(r)
}

#[inline]
fn round32(acc: u32, input: u32) -> u32 {
    rotl(acc.wrapping_add(input.wrapping_mul(PRIME32_2)), 13)
        .wrapping_mul(PRIME32_1)
}

#[inline]
fn round64(acc: u64, input: u64) -> u64 {
    rotl64(
        acc.wrapping_add(input.wrapping_mul(PRIME64_2)),
        31,
    )
    .wrapping_mul(PRIME64_1)
}

#[inline]
fn merge_round64(acc: u64, val: u64) -> u64 {
    let val = round64(0, val);
    let acc = acc ^ val;
    acc.wrapping_mul(PRIME64_1).wrapping_add(PRIME64_4)
}

/// Compute XXH32 of `data` with the given seed.
#[must_use]
pub fn xxhash32_seeded(data: &[u8], seed: u32) -> u32 {
    let len = data.len();

    let mut hash: u32;
    let tail_start: usize;

    if len >= 16 {
        let mut v1 = seed.wrapping_add(PRIME32_1).wrapping_add(PRIME32_2);
        let mut v2 = seed.wrapping_add(PRIME32_2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(PRIME32_1);

        let mut i = 0;
        while i + 16 <= len {
            let r0 = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            let r1 = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]);
            let r2 = u32::from_le_bytes([data[i + 8], data[i + 9], data[i + 10], data[i + 11]]);
            let r3 = u32::from_le_bytes([data[i + 12], data[i + 13], data[i + 14], data[i + 15]]);
            v1 = round32(v1, r0);
            v2 = round32(v2, r1);
            v3 = round32(v3, r2);
            v4 = round32(v4, r3);
            i += 16;
        }

        hash = rotl(v1, 1)
            .wrapping_add(rotl(v2, 7))
            .wrapping_add(rotl(v3, 12))
            .wrapping_add(rotl(v4, 18));
        tail_start = i;
    } else {
        hash = seed.wrapping_add(PRIME32_5);
        tail_start = 0;
    }

    hash = hash.wrapping_add(len as u32);
    finalize32(hash, &data[tail_start..])
}

fn finalize32(mut hash: u32, tail: &[u8]) -> u32 {
    let mut i = 0;
    while i + 4 <= tail.len() {
        let word = u32::from_le_bytes([tail[i], tail[i + 1], tail[i + 2], tail[i + 3]]);
        hash = hash.wrapping_add(word.wrapping_mul(PRIME32_3));
        hash = rotl(hash, 17).wrapping_mul(PRIME32_4);
        i += 4;
    }
    while i < tail.len() {
        hash = hash.wrapping_add(u32::from(tail[i]).wrapping_mul(PRIME32_5));
        hash = rotl(hash, 11).wrapping_mul(PRIME32_1);
        i += 1;
    }
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(PRIME32_2);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(PRIME32_3);
    hash ^= hash >> 16;
    hash
}

/// Compute `XXHash32` with seed=0.
#[must_use]
pub fn xxhash32(data: &[u8]) -> u32 {
    xxhash32_seeded(data, 0)
}

/// Compute `XXHash64` with the given seed.
#[must_use]
pub fn xxhash64_seeded(data: &[u8], seed: u64) -> u64 {
    let len = data.len();
    let mut hash: u64;
    let tail_start: usize;

    if len >= 32 {
        let mut v1 = seed.wrapping_add(PRIME64_1).wrapping_add(PRIME64_2);
        let mut v2 = seed.wrapping_add(PRIME64_2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(PRIME64_1);

        let mut i = 0;
        while i + 32 <= len {
            let r = |off: usize| {
                u64::from_le_bytes([
                    data[i + off],
                    data[i + off + 1],
                    data[i + off + 2],
                    data[i + off + 3],
                    data[i + off + 4],
                    data[i + off + 5],
                    data[i + off + 6],
                    data[i + off + 7],
                ])
            };
            v1 = round64(v1, r(0));
            v2 = round64(v2, r(8));
            v3 = round64(v3, r(16));
            v4 = round64(v4, r(24));
            i += 32;
        }

        hash = rotl64(v1, 1)
            .wrapping_add(rotl64(v2, 7))
            .wrapping_add(rotl64(v3, 12))
            .wrapping_add(rotl64(v4, 18));
        hash = merge_round64(hash, v1);
        hash = merge_round64(hash, v2);
        hash = merge_round64(hash, v3);
        hash = merge_round64(hash, v4);
        tail_start = len - (len % 32);
    } else {
        hash = seed.wrapping_add(PRIME64_5);
        tail_start = 0;
    }

    hash = hash.wrapping_add(len as u64);

    // Tail processing — matches C `XXH64_finalize`.
    let mut i = tail_start;
    while i + 8 <= len {
        let word = u64::from_le_bytes([
            data[i],
            data[i + 1],
            data[i + 2],
            data[i + 3],
            data[i + 4],
            data[i + 5],
            data[i + 6],
            data[i + 7],
        ]);
        let k1 = round64(0, word);
        hash ^= k1;
        hash = rotl64(hash, 27)
            .wrapping_mul(PRIME64_1)
            .wrapping_add(PRIME64_4);
        i += 8;
    }
    if i + 4 <= len {
        let word = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        hash ^= u64::from(word).wrapping_mul(PRIME64_1);
        hash = rotl64(hash, 23)
            .wrapping_mul(PRIME64_2)
            .wrapping_add(PRIME64_3);
        i += 4;
    }
    while i < len {
        hash ^= u64::from(data[i]).wrapping_mul(PRIME64_5);
        hash = rotl64(hash, 11).wrapping_mul(PRIME64_1);
        i += 1;
    }

    // Avalanche.
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(PRIME64_2);
    hash ^= hash >> 29;
    hash = hash.wrapping_mul(PRIME64_3);
    hash ^= hash >> 32;
    hash
}

/// Compute `XXHash64` with seed=0.
#[must_use]
pub fn xxhash64(data: &[u8]) -> u64 {
    xxhash64_seeded(data, 0)
}

/// ZSTD frame checksum: `XXHash64` truncated to 32 bits.
/// Matches `zstd_decompress.c:1052` (`(U32)XXH64_digest(...)`).
#[must_use]
pub fn zstd_frame_checksum(data: &[u8]) -> u32 {
    xxhash64(data) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zstd_frame_checksum_of_1mib_zeros_matches_fixture() {
        // From `~/src/external/zstd/tests/golden-decompression/rle-first-block.zst`:
        // decoded 1 MiB of zeros has stored checksum 0xE1163EF1.
        let data = vec![0u8; 1_048_576];
        assert_eq!(zstd_frame_checksum(&data), 0xE116_3EF1);
    }

    #[test]
    fn xxhash32_empty() {
        assert_eq!(xxhash32(b""), 0x02CC_5D05);
    }

    #[test]
    fn determinism() {
        let input = b"determinism test for xxhash";
        assert_eq!(xxhash32(input), xxhash32(input));
        assert_eq!(xxhash64(input), xxhash64(input));
        assert_eq!(zstd_frame_checksum(input), zstd_frame_checksum(input));
    }
}
