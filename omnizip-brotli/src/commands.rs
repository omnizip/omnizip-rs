//! Brotli insert-and-copy command construction (RFC 7932 §10.3).
//!
//! Ported from `brotli/src/enc/command.rs` (BSD-3-Clause). Translates
//! the LZ77-level (insert_len, copy_len, distance) tuple into the
//! 704-symbol insert-and-copy alphabet + extra bits + distance code.

#![forbid(unsafe_code)]

use crate::static_codes::{K_COPY_BASE, K_COPY_EXTRA, K_INS_BASE, K_INS_EXTRA};

/// Number of short distance codes (RFC 7932 §10.4).
pub const NUM_DISTANCE_SHORT_CODES: u32 = 16;

/// A parsed Brotli command — the LZ77 token after conversion to
/// Brotli's wire encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrotliCommand {
    /// Literals inserted before this command's copy.
    pub insert_len: u32,
    /// Length of the back-reference copy (≥ 2).
    pub copy_len: u32,
    /// 1-based back-reference distance.
    pub distance: u32,
    /// Whether the distance is from the previous command's distance
    /// (the "use last distance" flag in RFC 7932 §10.3).
    pub use_last_distance: bool,
    /// Decoded command symbol (0..703).
    pub cmd_prefix: u16,
    /// Decoded distance symbol (0..63 for the static tree).
    pub dist_prefix: u16,
    /// Extra bits for the distance code.
    pub dist_extra: u32,
}

/// `Log2FloorNonZero(n)` for `n > 0`: position of the highest set bit.
/// Mirrors `brotli/src/enc/util.rs`.
#[must_use]
pub fn log2_floor_non_zero(n: u64) -> u32 {
    debug_assert!(n > 0);
    63 - n.leading_zeros()
}

/// Map an insert length to its 5-bit code (RFC 7932 §10.3).
#[must_use]
pub fn get_insert_length_code(insertlen: usize) -> u16 {
    if insertlen < 6 {
        insertlen as u16
    } else if insertlen < 130 {
        let nbits = log2_floor_non_zero((insertlen - 2) as u64) - 1;
        (((nbits << 1) as usize) + ((insertlen - 2) >> nbits) + 2) as u16
    } else if insertlen < 2114 {
        log2_floor_non_zero((insertlen - 66) as u64) as u16 + 10
    } else if insertlen < 6210 {
        21
    } else if insertlen < 22594 {
        22
    } else {
        23
    }
}

/// Map a copy length to its 5-bit code (RFC 7932 §10.3).
#[must_use]
pub fn get_copy_length_code(copylen: usize) -> u16 {
    if copylen < 10 {
        (copylen - 2) as u16
    } else if copylen < 134 {
        let nbits = log2_floor_non_zero((copylen - 6) as u64) - 1;
        (((nbits << 1) as usize) + ((copylen - 6) >> nbits) + 4) as u16
    } else if copylen < 2118 {
        log2_floor_non_zero((copylen - 70) as u64) as u16 + 12
    } else {
        23
    }
}

/// Combine insert + copy codes into the 704-symbol command prefix
/// (RFC 7932 §10.3). Ported from `combine_length_codes`.
#[must_use]
pub fn combine_length_codes(inscode: u16, copycode: u16, use_last_distance: bool) -> u16 {
    let bits64 = (copycode & 0x7) | ((inscode & 0x7) << 3);
    if use_last_distance && inscode < 8 && copycode < 16 {
        if copycode < 8 {
            bits64
        } else {
            bits64 | 64
        }
    } else {
        let sub_offset = 2 * ((copycode >> 3) as i32 + 3 * (inscode >> 3) as i32);
        let offset = (sub_offset << 5) + 0x40 + (0x520d40i32 >> sub_offset & 0xc0);
        (offset as u16 as i32 | bits64 as i32) as u16
    }
}

/// Compute the command prefix code for a given (insert, copy,
/// use_last_distance).
#[must_use]
pub fn get_length_code(insertlen: usize, copylen: usize, use_last_distance: bool) -> u16 {
    let inscode = get_insert_length_code(insertlen);
    let copycode = get_copy_length_code(copylen);
    combine_length_codes(inscode, copycode, use_last_distance)
}

/// Initial distance cache values used by both the encoder and
/// decoder. The decoder's `dist_rb` is the reverse of this
/// (dist_rb = [16, 15, 11, 4]), but `ComputeDistanceCode` and the
/// decoder's `TakeDistanceFromRingBuffer` agree on the semantics.
pub const INITIAL_DIST_CACHE: [i32; 4] = [4, 11, 15, 16];

/// Compute the distance code for a raw distance given the current
/// dist cache. Ported from upstream `ComputeDistanceCode`.
///
/// Returns a value in 0..15 (short code, references the cache) or
/// 16+ (complex code, direct distance encoding).
#[must_use]
pub fn compute_distance_code(distance: u32, max_distance: u32, dist_cache: &[i32; 4]) -> u32 {
    if distance <= max_distance {
        let distance_plus_3 = distance.wrapping_add(3);
        let offset0 = distance_plus_3.wrapping_sub(dist_cache[0] as u32);
        let offset1 = distance_plus_3.wrapping_sub(dist_cache[1] as u32);
        if distance == dist_cache[0] as u32 {
            return 0;
        } else if distance == dist_cache[1] as u32 {
            return 1;
        } else if offset0 < 7 {
            return u32::try_from(0x0975_0468_i32 >> (4 * offset0) & 0xF).unwrap_or(16);
        } else if offset1 < 7 {
            return u32::try_from(0x0fdb_1ace_i32 >> (4 * offset1) & 0xF).unwrap_or(16);
        } else if distance == dist_cache[2] as u32 {
            return 2;
        } else if distance == dist_cache[3] as u32 {
            return 3;
        }
    }
    distance.wrapping_add(16).wrapping_sub(1)
}

/// Distance code prefix encoding (RFC 7932 §10.4).
///
/// Returns `(code, extra_bits_count, extra_bits_value)`.
#[must_use]
pub fn prefix_encode_copy_distance(
    distance_code: u32,
    num_direct_codes: u32,
    postfix_bits: u32,
) -> (u16, u32, u32) {
    if distance_code < NUM_DISTANCE_SHORT_CODES + num_direct_codes {
        return (distance_code as u16, 0, 0);
    }
    let postfix_bits_u64 = u64::from(postfix_bits);
    let num_direct_u64 = u64::from(num_direct_codes);
    let dist: u64 = (1u64 << (postfix_bits_u64 + 2))
        + (u64::from(distance_code) - u64::from(NUM_DISTANCE_SHORT_CODES) - num_direct_u64);
    let bucket = u64::from(log2_floor_non_zero(dist) - 1);
    let postfix_mask: u64 = (1u64 << postfix_bits_u64).wrapping_sub(1);
    let postfix = dist & postfix_mask;
    let prefix = (dist >> bucket) & 1;
    let offset = (2u64 + prefix) << bucket;
    let nbits = bucket - postfix_bits_u64;
    let code = ((nbits << 10)
        | (u64::from(NUM_DISTANCE_SHORT_CODES)
            + num_direct_u64
            + ((2 * (nbits - 1) + prefix) << postfix_bits_u64)
            + postfix)) as u16;
    let extra_bits = ((dist - offset) >> postfix_bits_u64) as u32;
    (code, nbits as u32, extra_bits)
}

/// Compute the Brotli command + distance prefix for a back-reference.
#[must_use]
pub fn make_command(insert_len: u32, copy_len: u32, distance: u32) -> BrotliCommand {
    let cmd_prefix = get_length_code(insert_len as usize, copy_len as usize, false);
    let (dist_prefix, dist_nbits, dist_extra_value) =
        prefix_encode_copy_distance(distance, 0, 0);
    BrotliCommand {
        insert_len,
        copy_len,
        distance,
        use_last_distance: false,
        cmd_prefix,
        dist_prefix,
        dist_extra: (u32::from(dist_nbits) << 24) | dist_extra_value,
    }
}

/// Get the insert-length base value for a given code (used by decoder).
#[must_use]
pub fn insert_base(code: usize) -> u32 {
    K_INS_BASE[code]
}

/// Get the insert-length extra-bits count for a given code.
#[must_use]
pub fn insert_extra(code: usize) -> u32 {
    K_INS_EXTRA[code]
}

/// Get the copy-length base value for a given code.
#[must_use]
pub fn copy_base(code: usize) -> u32 {
    K_COPY_BASE[code]
}

/// Get the copy-length extra-bits count for a given code.
#[must_use]
pub fn copy_extra(code: usize) -> u32 {
    K_COPY_EXTRA[code]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log2_floor_handles_basic_values() {
        assert_eq!(log2_floor_non_zero(1), 0);
        assert_eq!(log2_floor_non_zero(2), 1);
        assert_eq!(log2_floor_non_zero(3), 1);
        assert_eq!(log2_floor_non_zero(4), 2);
        assert_eq!(log2_floor_non_zero(7), 2);
        assert_eq!(log2_floor_non_zero(8), 3);
        assert_eq!(log2_floor_non_zero(255), 7);
        assert_eq!(log2_floor_non_zero(256), 8);
    }

    #[test]
    fn insert_length_code_buckets() {
        // 0..6 → identity
        for i in 0..6 {
            assert_eq!(get_insert_length_code(i), i as u16, "insert {i}");
        }
        // 130 → Log2Floor(130-66) + 10 = Log2Floor(64) + 10 = 6 + 10 = 16.
        assert_eq!(get_insert_length_code(130), 16);
        // 2114 → Log2Floor(2114-66) + 10 = Log2Floor(2048) + 10 = 11 + 10 = 21.
        assert_eq!(get_insert_length_code(2114), 21);
    }

    #[test]
    fn copy_length_code_buckets() {
        // copy 2..10 → identity - 2
        for (i, expected) in [(2, 0), (3, 1), (4, 2), (5, 3), (6, 4), (7, 5), (8, 6), (9, 7)] {
            assert_eq!(get_copy_length_code(i), expected, "copy {i}");
        }
        // copy 134 → Log2Floor(134-70) + 12 = Log2Floor(64) + 12 = 6 + 12 = 18.
        assert_eq!(get_copy_length_code(134), 18);
    }

    #[test]
    fn prefix_encode_short_distance() {
        // Distance codes 0..15 are short codes: code = distance_code,
        // no extra bits.
        for d in 0..16u32 {
            let (code, nbits, value) = prefix_encode_copy_distance(d, 0, 0);
            assert_eq!(code, d as u16, "distance {d}");
            assert_eq!(nbits, 0);
            assert_eq!(value, 0);
        }
    }

    #[test]
    fn prefix_encode_distance_16() {
        // Distance 16: just past the short codes. Encoded with extra bits.
        let (code, _nbits, _value) = prefix_encode_copy_distance(16, 0, 0);
        // Should produce code 16 + something.
        assert!(code >= 16);
    }

    #[test]
    fn make_command_smallest() {
        // Smallest non-trivial command: insert 0, copy 2 from distance 1.
        let cmd = make_command(0, 2, 1);
        assert_eq!(cmd.insert_len, 0);
        assert_eq!(cmd.copy_len, 2);
        assert_eq!(cmd.distance, 1);
        assert!(cmd.cmd_prefix < 704);
    }

    #[test]
    fn make_command_typical_text_match() {
        // "hello world hello" → insert 11, copy 4 from distance 11
        let cmd = make_command(11, 4, 11);
        assert_eq!(cmd.insert_len, 11);
        assert_eq!(cmd.copy_len, 4);
        assert_eq!(cmd.distance, 11);
        assert!(cmd.cmd_prefix < 704);
    }
}