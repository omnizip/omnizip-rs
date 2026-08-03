//! Hash helpers shared across omnizip-rs codecs.
//!
//! Several codecs (PPMd, ZPAQ, ZSTD, LZMA match finder) need a fast
//! hash of a byte slice or a sequence of bytes. Centralising the
//! implementations here avoids subtle divergence (e.g. PPMd7 used
//! FNV-1a while PPMd8 used DJB2; both are valid but the duplication
//! was a maintenance smell).
//!
//! All hashes here are **deterministic**: same input → same output
//! across runs, machines, and Rust versions.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

/// FNV-1a 32-bit hash. Used by PPMd7 context hashing.
#[must_use]
pub fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for &b in bytes {
        h ^= u32::from(b);
        h = h.wrapping_mul(16_777_619);
    }
    h
}

/// FNV-1a 32-bit with an order tag mixed in (PPMd context hash).
#[must_use]
pub fn fnv1a_32_tagged(order: u8, bytes: &[u8]) -> u32 {
    let mut h = fnv1a_32(bytes);
    h ^= u32::from(order).wrapping_mul(0x9E37_79B9);
    h ^= h >> 16;
    h
}

/// DJB2 32-bit hash. Used by PPMd8 context hashing.
#[must_use]
pub fn djb2_32(bytes: &[u8]) -> u32 {
    let mut h: u32 = 5381;
    for &b in bytes {
        h = h.wrapping_mul(33).wrapping_add(u32::from(b));
    }
    h
}

/// Mix in a tag (e.g. order) to a DJB2 hash, with a finaliser.
#[must_use]
pub fn djb2_32_tagged(order: u8, bytes: &[u8]) -> u32 {
    let mut h = djb2_32(bytes);
    h ^= u32::from(order).wrapping_mul(0x9E37_79B9);
    h ^= h >> 16;
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_known_values() {
        // FNV-1a 32-bit of empty input.
        assert_eq!(fnv1a_32(b""), 2_166_136_261);
        // FNV-1a 32-bit of "a" — known value.
        assert_eq!(fnv1a_32(b"a"), 0xE4_0C_29_2C);
        // FNV-1a 32-bit of "foobar".
        assert_eq!(fnv1a_32(b"foobar"), 0xBF_9C_F9_68);
    }

    #[test]
    fn djb2_known_values() {
        assert_eq!(djb2_32(b""), 5381);
        assert_eq!(djb2_32(b"a"), 177_670);
    }

    #[test]
    fn tagged_hashes_differ_from_untagged() {
        let untagged = fnv1a_32(b"abc");
        let tagged = fnv1a_32_tagged(0, b"abc");
        // Even with order=0 the tag mixer changes the output.
        assert_ne!(untagged, tagged);

        // Different orders produce different hashes.
        let o1 = fnv1a_32_tagged(1, b"abc");
        let o2 = fnv1a_32_tagged(2, b"abc");
        assert_ne!(o1, o2);
    }

    #[test]
    fn determinism() {
        assert_eq!(fnv1a_32(b"hello"), fnv1a_32(b"hello"));
        assert_eq!(djb2_32(b"hello"), djb2_32(b"hello"));
    }
}
