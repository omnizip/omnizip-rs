//! `XXHash` — re-export from `omnizip_codecs::xxhash`.
//!
//! The canonical implementation now lives in `omnizip-codecs` so
//! other codecs can share it. See `TODO.complete/96-shared-xxhash.md`.

#![forbid(unsafe_code)]

pub use omnizip_codecs::xxhash::{
    xxhash32, xxhash32_seeded, xxhash64, xxhash64_seeded, zstd_frame_checksum,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn determinism() {
        assert_eq!(xxhash64(b"hello"), xxhash64(b"hello"));
    }

    #[test]
    fn xxhash32_empty() {
        // Known value: XXH32("", seed=0) = 0x02CC5D05.
        assert_eq!(xxhash32(b""), 0x02CC_5D05);
    }

    #[test]
    fn zstd_frame_checksum_of_1mib_zeros_matches_fixture() {
        // From `~/src/external/zstd/tests/golden-decompression/rle-first-block.zst`:
        // decoded 1 MiB of zeros has stored checksum 0xE1163EF1.
        let zeros = vec![0u8; 1_048_576];
        assert_eq!(zstd_frame_checksum(&zeros), 0xE116_3EF1);
    }
}
