//! Byte-shuffle and bit-shuffle filters.
//!
//! Both filters are transposition transforms that reorder data to expose
//! redundancy for downstream codecs (LZ4, ZSTD, Brotli, ...):
//!
//! - **ByteShuffle** — given `item_size = N`, for a block of `K` items
//!   (each `N` bytes), transpose so all byte-0s are adjacent, then all
//!   byte-1s, etc. Excellent for arrays of struct-of-records where each
//!   byte lane carries similar values across items (e.g. f32 mantissa
//!   bytes).
//! - **BitShuffle** — same idea but transposes at the bit level within
//!   each group of 8 items. Bit-0 of all 8 items becomes contiguous,
//!   then bit-1, etc. Stronger redundancy exposure than byte shuffle
//!   for low-entropy scientific data.
//!
//! ## Wire format
//!
//! Both filters are self-describing on the wire — the filter kind and
//! the item size are written as a 2-byte prefix so the decoder can be
//! the parameterless [`Filter::decode`]:
//!
//! ```text
//! [0x00 = byte shuffle | 0x01 = bit shuffle] [item_size: u8] [shuffled data]
//! ```
//!
//! Trailing bytes that do not fit a complete item (i.e. when the input
//! length is not a multiple of the relevant unit) are emitted unchanged
//! at the tail of the shuffled data, so the round-trip is exact for
//! every input.
//!
//! ## Determinism and reversibility
//!
//! Both filters are fully deterministic (same input ⇒ byte-identical
//! output) and exactly reversible
//! (`filter.decode(filter.encode(data)) == data` for all inputs).

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use crate::Filter;

/// Item sizes accepted by both shuffle filters. The XZ / ZSTD shuffle
/// convention restricts to powers of two in `{1, 2, 4, 8}`.
pub const VALID_ITEM_SIZES: [usize; 4] = [1, 2, 4, 8];

/// Wire tag for the byte-shuffle filter.
const TAG_BYTE_SHUFFLE: u8 = 0x00;
/// Wire tag for the bit-shuffle filter.
const TAG_BIT_SHUFFLE: u8 = 0x01;

/// Verify `item_size` is one of the accepted values.
fn check_item_size(item_size: usize) {
    assert!(
        VALID_ITEM_SIZES.contains(&item_size),
        "item_size must be one of {VALID_ITEM_SIZES:?}, got {item_size}",
    );
}

/// Convert a validated item size to its `u8` wire representation.
fn item_size_to_u8(item_size: usize) -> u8 {
    debug_assert!(
        VALID_ITEM_SIZES.contains(&item_size),
        "item_size must be one of {VALID_ITEM_SIZES:?}",
    );
    #[allow(clippy::cast_possible_truncation)]
    {
        item_size as u8
    }
}

/// Split `data` into `(aligned_prefix, trailing_tail)` where
/// `aligned_prefix.len()` is a whole multiple of `unit`.
fn split_aligned(data: &[u8], unit: usize) -> (&[u8], &[u8]) {
    let boundary = data.len() / unit * unit;
    data.split_at(boundary)
}

/// The 2-byte wire header (`[tag] [item_size]`).
const HEADER_LEN: usize = 2;

// ---------------------------------------------------------------------------
// Byte shuffle
// ---------------------------------------------------------------------------

/// Byte-shuffle filter.
///
/// For a block of `K` items of `item_size` bytes each, the encoded form
/// transposes bytes so all byte-lane-0 bytes are contiguous, then all
/// byte-lane-1 bytes, etc.
///
/// Trailing bytes that do not form a complete item pass through
/// unchanged at the tail.
pub struct ByteShuffle {
    item_size: usize,
}

impl ByteShuffle {
    /// Construct a byte-shuffle filter with the given item size.
    ///
    /// # Panics
    ///
    /// Panics if `item_size` is not one of `{1, 2, 4, 8}`.
    #[must_use]
    pub fn new(item_size: usize) -> Self {
        check_item_size(item_size);
        Self { item_size }
    }

    /// The configured item size in bytes.
    #[must_use]
    pub fn item_size(&self) -> usize {
        self.item_size
    }
}

impl Filter for ByteShuffle {
    fn name(&self) -> &'static str {
        "byte-shuffle"
    }

    fn encode(&self, input: &[u8]) -> Vec<u8> {
        let item_size = self.item_size;
        let mut out = Vec::with_capacity(input.len() + HEADER_LEN);
        out.push(TAG_BYTE_SHUFFLE);
        out.push(item_size_to_u8(item_size));

        let (body, tail) = split_aligned(input, item_size);
        let num_items = body.len() / item_size;

        // Lane k (0..item_size) collects byte k of every item, occupying
        // positions [k * num_items .. (k+1) * num_items).
        let body_start = out.len();
        out.resize(body_start + body.len(), 0);
        for (item_idx, item) in body.chunks_exact(item_size).enumerate() {
            for (lane_idx, &byte) in item.iter().enumerate() {
                out[body_start + lane_idx * num_items + item_idx] = byte;
            }
        }

        // Trailing partial-item bytes pass through unchanged.
        out.extend_from_slice(tail);
        out
    }

    fn decode(&self, input: &[u8]) -> Vec<u8> {
        if input.is_empty() {
            return Vec::new();
        }
        let (tag, item_size, body) = read_header(input, TAG_BYTE_SHUFFLE);
        debug_assert_eq!(tag, TAG_BYTE_SHUFFLE);

        let (aligned, tail) = split_aligned(body, item_size);
        let num_items = aligned.len() / item_size;

        let mut out = vec![0u8; aligned.len()];
        for (item_idx, item_slot) in out.chunks_exact_mut(item_size).enumerate() {
            for (lane_idx, byte_slot) in item_slot.iter_mut().enumerate() {
                *byte_slot = aligned[lane_idx * num_items + item_idx];
            }
        }
        out.extend_from_slice(tail);
        out
    }
}

// ---------------------------------------------------------------------------
// Bit shuffle
// ---------------------------------------------------------------------------

/// Bit-shuffle filter.
///
/// For each complete group of 8 items (each `item_size` bytes), the
/// encoded form transposes the bits so bit-0 of every byte in every
/// item is contiguous across the 8 items, then bit-1, etc. Trailing
/// items that do not fill a complete group of 8 are emitted unchanged.
///
/// Within a group of 8 items, the transpose is performed bit-by-bit
/// across all `8 * item_size` bytes of the group. The 8×8 bit transpose
/// is its own inverse, so the decode path uses the same routine.
pub struct BitShuffle {
    item_size: usize,
}

impl BitShuffle {
    /// Construct a bit-shuffle filter with the given item size.
    ///
    /// # Panics
    ///
    /// Panics if `item_size` is not one of `{1, 2, 4, 8}`.
    #[must_use]
    pub fn new(item_size: usize) -> Self {
        check_item_size(item_size);
        Self { item_size }
    }

    /// The configured item size in bytes.
    #[must_use]
    pub fn item_size(&self) -> usize {
        self.item_size
    }
}

impl Filter for BitShuffle {
    fn name(&self) -> &'static str {
        "bit-shuffle"
    }

    fn encode(&self, input: &[u8]) -> Vec<u8> {
        let item_size = self.item_size;
        let group_bytes = 8 * item_size;
        let mut out = Vec::with_capacity(input.len() + HEADER_LEN);
        out.push(TAG_BIT_SHUFFLE);
        out.push(item_size_to_u8(item_size));

        let (grouped, remainder) = split_aligned(input, group_bytes);

        let body_start = out.len();
        out.resize(body_start + grouped.len(), 0);
        for (group_idx, group) in grouped.chunks_exact(group_bytes).enumerate() {
            let group_start = body_start + group_idx * group_bytes;
            transpose_bits_group(group, &mut out[group_start..group_start + group_bytes]);
        }

        // Items that did not fill a complete group pass through unchanged.
        out.extend_from_slice(remainder);
        out
    }

    fn decode(&self, input: &[u8]) -> Vec<u8> {
        if input.is_empty() {
            return Vec::new();
        }
        let (tag, item_size, body) = read_header(input, TAG_BIT_SHUFFLE);
        debug_assert_eq!(tag, TAG_BIT_SHUFFLE);

        let group_bytes = 8 * item_size;
        let (grouped, remainder) = split_aligned(body, group_bytes);

        // The 8×8 bit transpose is self-inverse, so decode of each group
        // is identical to encode.
        let mut out = vec![0u8; grouped.len()];
        for (group_idx, group) in grouped.chunks_exact(group_bytes).enumerate() {
            let group_start = group_idx * group_bytes;
            transpose_bits_group(group, &mut out[group_start..group_start + group_bytes]);
        }
        out.extend_from_slice(remainder);
        out
    }
}

/// Bit-transpose a single group of exactly 8 items.
///
/// For a group of `8 * item_size` bytes, route every bit at position
/// `(item_idx in 0..8, byte_in_item in 0..item_size, bit_in_byte in
/// 0..8)` to output byte index `bit_in_byte * item_size +
/// byte_in_item`, MSB-first within that byte across the 8 items.
///
/// This 8×8 transpose is self-inverse, so the same routine is used for
/// encode and decode.
fn transpose_bits_group(input: &[u8], output: &mut [u8]) {
    debug_assert_eq!(input.len(), output.len());
    debug_assert!(!input.is_empty());
    debug_assert_eq!(input.len() % 8, 0);

    let item_size = input.len() / 8;
    output.fill(0);

    for (item_idx, item) in input.chunks_exact(item_size).enumerate() {
        for (byte_in_item, &byte) in item.iter().enumerate() {
            for bit_in_byte in 0..8usize {
                let bit_val = (byte >> (7 - bit_in_byte)) & 1;
                if bit_val != 0 {
                    let output_byte = bit_in_byte * item_size + byte_in_item;
                    let output_bit_within_byte = 7 - item_idx;
                    output[output_byte] |= 1u8 << output_bit_within_byte;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Header decode
// ---------------------------------------------------------------------------

/// Read the 2-byte wire header, validating the tag and item size.
///
/// Returns `(tag, item_size, body)` where `body` is everything after
/// the header.
fn read_header(input: &[u8], expected_tag: u8) -> (u8, usize, &[u8]) {
    assert!(
        input.len() >= HEADER_LEN,
        "shuffle payload too short: need at least {HEADER_LEN} header bytes, got {}",
        input.len(),
    );
    let tag = input[0];
    assert!(
        tag == expected_tag,
        "shuffle tag mismatch: expected {expected_tag:#04x}, got {tag:#04x}",
    );
    let item_size = usize::from(input[1]);
    assert!(
        VALID_ITEM_SIZES.contains(&item_size),
        "shuffle item_size on wire must be one of {VALID_ITEM_SIZES:?}, got {item_size}",
    );
    (tag, item_size, &input[HEADER_LEN..])
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Deterministic pseudo-random data (no RNG seed needed; the LCG is
    // fixed).
    // -----------------------------------------------------------------
    fn lcg_bytes(n: usize, seed: u64) -> Vec<u8> {
        let mut state = seed;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            out.push((state >> 56) as u8);
        }
        out
    }

    fn round_trip<F: Filter>(filter: &F, data: &[u8]) {
        let encoded = filter.encode(data);
        let decoded = filter.decode(&encoded);
        assert_eq!(
            decoded.as_slice(),
            data,
            "round-trip mismatch for {}",
            filter.name()
        );
    }

    // -----------------------------------------------------------------
    // ByteShuffle — round trips
    // -----------------------------------------------------------------
    #[test]
    fn byte_shuffle_round_trips_each_item_size_on_random_data() {
        for &item_size in &VALID_ITEM_SIZES {
            let filter = ByteShuffle::new(item_size);
            let data = lcg_bytes(1024, 0xC0FFEE);
            assert_eq!(
                data.len() % item_size,
                0,
                "test invariant: data length must be a multiple of item_size {item_size}",
            );
            round_trip(&filter, &data);
        }
    }

    #[test]
    fn byte_shuffle_round_trips_unaligned_data() {
        for &item_size in &VALID_ITEM_SIZES {
            let filter = ByteShuffle::new(item_size);
            // Pick lengths that produce a non-empty partial tail for each
            // item_size.
            for &extra in &[1usize, item_size - 1, 3, 7] {
                if extra == 0 || extra >= item_size {
                    continue;
                }
                let len = 16 * item_size + extra;
                let data = lcg_bytes(len, 0xABCDEF);
                round_trip(&filter, &data);
            }
        }
    }

    #[test]
    fn byte_shuffle_handles_empty_input() {
        for &item_size in &VALID_ITEM_SIZES {
            let filter = ByteShuffle::new(item_size);
            let encoded = filter.encode(b"");
            assert_eq!(
                encoded.len(),
                HEADER_LEN,
                "wire header should be {HEADER_LEN} bytes, got {}",
                encoded.len(),
            );
            assert_eq!(filter.decode(&encoded), b"");
        }
    }

    #[test]
    fn byte_shuffle_handles_single_item() {
        for &item_size in &VALID_ITEM_SIZES {
            let filter = ByteShuffle::new(item_size);
            let data = lcg_bytes(item_size, 0x11);
            round_trip(&filter, &data);
        }
    }

    #[test]
    fn byte_shuffle_layout_is_transposed() {
        // Two 4-byte items: [a0,a1,a2,a3, b0,b1,b2,b3]
        // must transpose to  [a0,b0, a1,b1, a2,b2, a3,b3].
        let filter = ByteShuffle::new(4);
        let data = [0x10u8, 0x11, 0x12, 0x13, 0x20, 0x21, 0x22, 0x23];
        let encoded = filter.encode(&data);
        let body = &encoded[HEADER_LEN..];
        assert_eq!(
            body,
            &[
                0x10, 0x20, // lane 0 (first byte of every item)
                0x11, 0x21, // lane 1
                0x12, 0x22, // lane 2
                0x13, 0x23,
            ],
        );
    }

    #[test]
    #[should_panic(expected = "item_size must be one of")]
    fn byte_shuffle_rejects_invalid_item_size() {
        let _ = ByteShuffle::new(3);
    }

    #[test]
    fn byte_shuffle_partial_tail_passes_through() {
        // 5 bytes with item_size 4 → one full item + one tail byte.
        let filter = ByteShuffle::new(4);
        let data = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
        let encoded = filter.encode(&data);
        assert_eq!(encoded[HEADER_LEN..].len(), data.len());
        // The 5th byte must appear unchanged at the tail.
        assert_eq!(encoded[HEADER_LEN + 4], 0xEE);
    }

    // -----------------------------------------------------------------
    // BitShuffle — round trips
    // -----------------------------------------------------------------
    #[test]
    fn bit_shuffle_round_trips_each_item_size_on_random_data() {
        for &item_size in &VALID_ITEM_SIZES {
            let filter = BitShuffle::new(item_size);
            // Use a length that is a whole multiple of 8 * item_size so
            // no trailing partial group exists.
            let len = 8 * item_size * 16;
            let data = lcg_bytes(len, 0xFEED);
            round_trip(&filter, &data);
        }
    }

    #[test]
    fn bit_shuffle_round_trips_with_partial_tail() {
        for &item_size in &VALID_ITEM_SIZES {
            let filter = BitShuffle::new(item_size);
            let group_bytes = 8 * item_size;
            for &extra in &[1usize, item_size, group_bytes - 1] {
                if extra >= group_bytes {
                    continue;
                }
                let len = 4 * group_bytes + extra;
                let data = lcg_bytes(len, 0x1234);
                round_trip(&filter, &data);
            }
        }
    }

    #[test]
    fn bit_shuffle_handles_empty_input() {
        for &item_size in &VALID_ITEM_SIZES {
            let filter = BitShuffle::new(item_size);
            let encoded = filter.encode(b"");
            assert_eq!(encoded.len(), HEADER_LEN);
            assert_eq!(filter.decode(&encoded), b"");
        }
    }

    #[test]
    fn bit_shuffle_single_group_round_trips() {
        for &item_size in &VALID_ITEM_SIZES {
            let filter = BitShuffle::new(item_size);
            let data = lcg_bytes(8 * item_size, 0x77);
            round_trip(&filter, &data);
        }
    }

    #[test]
    fn bit_shuffle_all_zero_input_stays_zero() {
        let filter = BitShuffle::new(4);
        let data = vec![0u8; 64];
        let encoded = filter.encode(&data);
        assert_eq!(&encoded[HEADER_LEN..], &data[..]);
    }

    #[test]
    fn bit_shuffle_all_ones_input_stays_all_ones() {
        let filter = BitShuffle::new(4);
        let data = vec![0xFFu8; 64];
        let encoded = filter.encode(&data);
        assert_eq!(&encoded[HEADER_LEN..], &data[..]);
    }

    #[test]
    fn bit_shuffle_item_size_one_swaps_bitlanes() {
        // 8 bytes whose top bit (bit_in_byte=0) is set only in items
        // 0, 2, 4:
        //   0x80 0x00 0x80 0x00 0x80 0x00 0x00 0x00
        // After transpose, bit 0 (the top bit) of every item lives in
        // output byte index 0. Output bit `7 - item_idx` within that
        // byte is set for items 0, 2, 4 → bits 7, 5, 3 → 0b1010_1000 =
        // 0xA8. All other output bytes are zero.
        let filter = BitShuffle::new(1);
        let data = [0x80u8, 0x00, 0x80, 0x00, 0x80, 0x00, 0x00, 0x00];
        let encoded = filter.encode(&data);
        assert_eq!(
            &encoded[HEADER_LEN..],
            &[0xA8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );
    }

    // -----------------------------------------------------------------
    // Cross-filter sanity
    // -----------------------------------------------------------------
    #[test]
    fn wire_tags_are_distinct() {
        assert_ne!(TAG_BYTE_SHUFFLE, TAG_BIT_SHUFFLE);
    }

    #[test]
    fn byte_and_bit_shuffle_produce_distinct_output_on_same_input() {
        let data = lcg_bytes(64, 0xBEEF);
        let bs = ByteShuffle::new(4).encode(&data);
        let bts = BitShuffle::new(4).encode(&data);
        // Headers differ in the tag byte.
        assert_ne!(bs[0], bts[0]);
        // Bodies almost certainly differ on random data.
        assert_ne!(bs[HEADER_LEN..], bts[HEADER_LEN..]);
    }

    // -----------------------------------------------------------------
    // Compressibility: shuffled f32 data should compress better with LZ4
    // than unshuffled f32 data.
    // -----------------------------------------------------------------
    #[test]
    fn byte_shuffle_improves_lz4_compression_on_floats() {
        // Generate a sequence of f32 values that are highly similar to
        // their neighbours (a smooth ramp with small per-step jitter).
        // This pattern compresses better after byte-shuffle because each
        // byte lane becomes near-constant.
        let n = 1024usize;
        let mut raw: Vec<u8> = Vec::with_capacity(n * 4);
        for i in 0..n {
            let f = (i as f32) * 0.5 + ((i as f32).sin() * 16.0);
            raw.extend_from_slice(&f.to_bits().to_le_bytes());
        }

        let shuffled = ByteShuffle::new(4).encode(&raw);
        let shuffled_body = &shuffled[HEADER_LEN..]; // strip header

        // Compress both with the in-house LZ4 block encoder.
        // We compare compressed sizes directly.
        fn lz4_compress(data: &[u8]) -> Vec<u8> {
            let block = omnizip_lz4::block::compress_block(data);
            let mut out = Vec::with_capacity(4 + block.len());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&block);
            out
        }
        let raw_compressed = lz4_compress(&raw);
        let shuffled_compressed = lz4_compress(shuffled_body);

        assert!(
            shuffled_compressed.len() <= raw_compressed.len(),
            "byte-shuffled float data should compress at least as well with LZ4: \
             shuffled_compressed={} raw_compressed={}",
            shuffled_compressed.len(),
            raw_compressed.len(),
        );
    }
}
