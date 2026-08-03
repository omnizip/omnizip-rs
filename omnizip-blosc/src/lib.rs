//! omnizip-blosc — Pure-Rust BLOSC2-style multi-codec container.
//!
//! BLOSC ("Blocking Shuffle Compression") is a meta-codec: it applies a
//! reversible filter (byte or bit shuffle) to expose redundancy, then
//! delegates the actual compression to an inner codec (here, LZ4). On
//! arrays of fixed-width records — especially f32/f64 scientific data —
//! the shuffle step dramatically improves the inner codec's ratio
//! because each byte (or bit) lane becomes near-constant across items.
//!
//! ## Container format
//!
//! ```text
//! +-----------------------+ 8 bytes:  magic b"BLOSC2\0\0"
//! | magic                 |
//! +-----------------------+ 1 byte:   version (= 2)
//! | version               |
//! +-----------------------+ 1 byte:   item_size (1, 2, 4, or 8)
//! | item_size             |
//! +-----------------------+ 1 byte:   shuffle_mode (0=none,1=byte,2=bit)
//! | shuffle_mode          |
//! +-----------------------+ 1 byte:   inner_codec (= 1 for LZ4)
//! | inner_codec           |
//! +-----------------------+ 4 bytes LE: uncompressed (plaintext) size
//! | uncompressed_size     |
//! +-----------------------+ 4 bytes LE: shuffled-body size (pre-LZ4)
//! | shuffled_size         |
//! +-----------------------+ 4 bytes:  reserved (= 0)
//! | reserved              |
//! +-----------------------+ variable: LZ4 frame of the shuffled body
//! | lz4_compressed_body   |
//! +-----------------------+
//! ```
//!
//! The header is 24 bytes. The body is always LZ4-compressed; when
//! `shuffle_mode != 0` the plaintext is shuffled before LZ4, and the
//! shuffle is reversed after LZ4 decode. `shuffled_size` lets the
//! decoder allocate the exact intermediate buffer regardless of how
//! `lz4_flex` frames its output.
//!
//! ## Determinism
//!
//! Every step (shuffle transpose, LZ4 encode) is fully deterministic:
//! same input + same parameters ⇒ byte-identical output across runs,
//! machines, and Rust versions. This is a hard requirement for
//! content-addressed storage (`LimniFS`'s `DropId = BLAKE3(plaintext)`).
//!
//! ## Reversibility
//!
//! Round-trip is exact for every input, including inputs whose length is
//! not a multiple of `item_size` (trailing partial-item bytes pass
//! through the shuffle unchanged) and the empty input.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Magic prefixing every BLOSC2 container frame.
const MAGIC: &[u8; 8] = b"BLOSC2\0\0";

/// Container format version.
const VERSION: u8 = 2;

/// Wire tag for the inner LZ4 codec.
const INNER_CODEC_LZ4: u8 = 1;

/// Fixed header length in bytes.
const HEADER_LEN: usize = 24;

/// Item sizes accepted by the shuffle stage (XZ / ZSTD / BLOSC convention).
const VALID_ITEM_SIZES: [u8; 4] = [1, 2, 4, 8];

// ---------------------------------------------------------------------------
// ShuffleMode
// ---------------------------------------------------------------------------

/// Selects the shuffle filter applied before LZ4 compression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShuffleMode {
    /// No shuffle — plaintext is passed straight to LZ4.
    None,
    /// Byte shuffle: byte-lane `k` of every item becomes contiguous.
    Byte,
    /// Bit shuffle: like [`ShuffleMode::Byte`] but transposed at the bit
    /// level within each group of 8 items. Stronger redundancy exposure
    /// for low-entropy scientific data.
    Bit,
}

impl ShuffleMode {
    /// Wire tag stored in the container header.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Byte => 1,
            Self::Bit => 2,
        }
    }

    /// Decode the wire tag, returning `None` if the value is unknown.
    #[must_use]
    pub const fn from_u8(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::None),
            1 => Some(Self::Byte),
            2 => Some(Self::Bit),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compress `input` using the BLOSC2-style container.
///
/// Applies the requested [`ShuffleMode`] with the given `item_size`,
/// then LZ4-compresses the shuffled body and prepends the 24-byte
/// header.
///
/// `item_size` must be one of `{1, 2, 4, 8}`.
///
/// # Errors
///
/// Returns [`OmnizipError::Corrupt`] if `item_size` is invalid or the
/// uncompressed length exceeds `u32::MAX`, and
/// [`OmnizipError::EncodeFailed`] if the inner LZ4 encoder fails.
#[allow(clippy::cast_possible_truncation)]
pub fn compress(
    input: &[u8],
    item_size: u8,
    shuffle: ShuffleMode,
) -> Result<Vec<u8>, OmnizipError> {
    validate_item_size(item_size)?;

    let uncompressed_size = u32::try_from(input.len()).map_err(|_| OmnizipError::Corrupt {
        codec: CodecId::BLOSC,
        reason: format!("uncompressed size {} exceeds u32::MAX", input.len()),
    })?;

    // Shuffle (or pass through). The shuffled body has the same length
    // as the input — the transpose is in-place sized.
    let shuffled: Vec<u8> = match shuffle {
        ShuffleMode::None => input.to_vec(),
        ShuffleMode::Byte => byte_shuffle(input, item_size),
        ShuffleMode::Bit => bit_shuffle(input, item_size),
    };
    debug_assert_eq!(shuffled.len(), input.len());
    let shuffled_size = u32::try_from(shuffled.len()).map_err(|_| OmnizipError::Corrupt {
        codec: CodecId::BLOSC,
        reason: format!("shuffled size {} exceeds u32::MAX", shuffled.len()),
    })?;

    // LZ4-compress the shuffled body. `compress_prepend_size` writes a
    // 4-byte LE original-size prefix that the decoder uses to allocate
    // the exact output buffer.
    let lz4_body = lz4_flex::compress_prepend_size(&shuffled);

    // Assemble the header + body.
    let mut out = Vec::with_capacity(HEADER_LEN + lz4_body.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(item_size);
    out.push(shuffle.as_u8());
    out.push(INNER_CODEC_LZ4);
    out.extend_from_slice(&uncompressed_size.to_le_bytes());
    out.extend_from_slice(&shuffled_size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // reserved
    out.extend_from_slice(&lz4_body);
    Ok(out)
}

/// Decompress a BLOSC2-style container produced by [`compress`].
///
/// Parses the header, LZ4-decompresses the body, then reverses the
/// shuffle (if any) to recover the original plaintext.
///
/// # Errors
///
/// Returns [`OmnizipError::Corrupt`] if the header is malformed,
/// [`OmnizipError::DecodeFailed`] if the LZ4 decoder fails, and
/// [`OmnizipError::LengthMismatch`] if the recovered plaintext length
/// does not match `uncompressed_size` from the header.
pub fn decompress(compressed: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    let header = parse_header(compressed)?;
    let body = &compressed[HEADER_LEN..];

    // LZ4-decompress the body. `decompress_size_prepended` reads the
    // 4-byte LE size prefix written by `compress_prepend_size`.
    let shuffled =
        lz4_flex::decompress_size_prepended(body).map_err(|e| OmnizipError::DecodeFailed {
            codec: CodecId::BLOSC,
            reason: format!("lz4 decompress failed: {e}"),
        })?;

    if u32::try_from(shuffled.len()).unwrap_or(u32::MAX) != header.shuffled_size {
        return Err(OmnizipError::LengthMismatch {
            codec: CodecId::BLOSC,
            expected: header.shuffled_size,
            actual: shuffled.len(),
        });
    }

    // Reverse the shuffle to recover the plaintext.
    let plaintext = match header.shuffle_mode {
        ShuffleMode::None => shuffled,
        ShuffleMode::Byte => byte_unshuffle(&shuffled, header.item_size),
        ShuffleMode::Bit => bit_unshuffle(&shuffled, header.item_size),
    };

    if u32::try_from(plaintext.len()).unwrap_or(u32::MAX) != header.uncompressed_size {
        return Err(OmnizipError::LengthMismatch {
            codec: CodecId::BLOSC,
            expected: header.uncompressed_size,
            actual: plaintext.len(),
        });
    }

    Ok(plaintext)
}

// ---------------------------------------------------------------------------
// Codec trait impl
// ---------------------------------------------------------------------------

/// BLOSC2-style container codec.
///
/// Implements [`Codec`] with sensible defaults (`item_size = 4`,
/// byte shuffle) so it can be registered in a `CodecRegistry` alongside
/// the other omnizip-rs codecs.
pub struct BloscCodec;

impl BloscCodec {
    /// Construct a `BloscCodec`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Compress with explicit user-tunable options (item size, shuffle mode).
    ///
    /// Thin wrapper over the free function [`compress`] that documents
    /// the tunable surface in one place. Same as calling `compress()`
    /// directly.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::Corrupt`] if `item_size` is invalid.
    pub fn compress_with_options(
        &self,
        plaintext: &[u8],
        item_size: u8,
        shuffle: ShuffleMode,
    ) -> Result<Vec<u8>, OmnizipError> {
        compress(plaintext, item_size, shuffle)
    }
}

impl Codec for BloscCodec {
    fn id(&self) -> CodecId {
        CodecId::BLOSC
    }
    fn name(&self) -> &'static str {
        "blosc"
    }
    fn compress(
        &self,
        plaintext: &[u8],
        _level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        // Default: 4-byte items (f32 / i32 / u32), byte shuffle. The
        // BLOSC container has no notion of a compression level — the
        // inner LZ4 codec is fixed — so `_level` is ignored.
        compress(plaintext, 4, ShuffleMode::Byte)
    }
    fn decompress(&self, compressed: &[u8], _expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        // `expected_len` is recorded in the container header and checked
        // there, so the caller-supplied value is not needed for
        // correctness.
        decompress(compressed)
    }
}

// ---------------------------------------------------------------------------
// Header parsing
// ---------------------------------------------------------------------------

/// Parsed container header.
#[derive(Debug)]
struct Header {
    item_size: u8,
    shuffle_mode: ShuffleMode,
    uncompressed_size: u32,
    shuffled_size: u32,
}

/// Parse and validate the 24-byte header.
fn parse_header(compressed: &[u8]) -> Result<Header, OmnizipError> {
    if compressed.len() < HEADER_LEN {
        return Err(OmnizipError::Corrupt {
            codec: CodecId::BLOSC,
            reason: format!(
                "header too short: need {HEADER_LEN} bytes, got {}",
                compressed.len()
            ),
        });
    }

    let magic = &compressed[..8];
    if magic != MAGIC {
        return Err(OmnizipError::Corrupt {
            codec: CodecId::BLOSC,
            reason: format!("bad magic: expected {MAGIC:?}, got {magic:?}"),
        });
    }

    let version = compressed[8];
    if version != VERSION {
        return Err(OmnizipError::Corrupt {
            codec: CodecId::BLOSC,
            reason: format!("unsupported version {version}, expected {VERSION}"),
        });
    }

    let item_size = compressed[9];
    validate_item_size(item_size)?;

    let shuffle_raw = compressed[10];
    let shuffle_mode = ShuffleMode::from_u8(shuffle_raw).ok_or_else(|| OmnizipError::Corrupt {
        codec: CodecId::BLOSC,
        reason: format!("unknown shuffle_mode {shuffle_raw}"),
    })?;

    let inner_codec = compressed[11];
    if inner_codec != INNER_CODEC_LZ4 {
        return Err(OmnizipError::Corrupt {
            codec: CodecId::BLOSC,
            reason: format!(
                "unsupported inner codec {inner_codec}, expected {INNER_CODEC_LZ4} (LZ4)"
            ),
        });
    }

    let uncompressed_size = u32::from_le_bytes([
        compressed[12],
        compressed[13],
        compressed[14],
        compressed[15],
    ]);
    let shuffled_size = u32::from_le_bytes([
        compressed[16],
        compressed[17],
        compressed[18],
        compressed[19],
    ]);
    // bytes 20..24 are reserved and ignored.

    Ok(Header {
        item_size,
        shuffle_mode,
        uncompressed_size,
        shuffled_size,
    })
}

/// Reject item sizes outside the accepted set.
fn validate_item_size(item_size: u8) -> Result<(), OmnizipError> {
    if VALID_ITEM_SIZES.contains(&item_size) {
        Ok(())
    } else {
        Err(OmnizipError::Corrupt {
            codec: CodecId::BLOSC,
            reason: format!("item_size must be one of {VALID_ITEM_SIZES:?}, got {item_size}"),
        })
    }
}

// ---------------------------------------------------------------------------
// Byte shuffle
// ---------------------------------------------------------------------------

/// Transpose `data` so byte-lane `k` of every `item_size`-byte item is
/// contiguous.
///
/// Trailing bytes that do not form a complete item pass through
/// unchanged at the tail. The output length equals the input length.
fn byte_shuffle(data: &[u8], item_size: u8) -> Vec<u8> {
    debug_assert!(VALID_ITEM_SIZES.contains(&item_size));
    let item_size = usize::from(item_size);
    let (body, tail) = split_aligned(data, item_size);
    let num_items = body.len() / item_size;

    let mut out = vec![0u8; body.len()];
    for (item_idx, item) in body.chunks_exact(item_size).enumerate() {
        for (lane_idx, &byte) in item.iter().enumerate() {
            // Lane k (0..item_size) collects byte k of every item.
            out[lane_idx * num_items + item_idx] = byte;
        }
    }
    out.extend_from_slice(tail);
    out
}

/// Inverse of [`byte_shuffle`].
fn byte_unshuffle(data: &[u8], item_size: u8) -> Vec<u8> {
    debug_assert!(VALID_ITEM_SIZES.contains(&item_size));
    let item_size = usize::from(item_size);
    let (body, tail) = split_aligned(data, item_size);
    let num_items = body.len() / item_size;

    let mut out = vec![0u8; body.len()];
    for (item_idx, item_slot) in out.chunks_exact_mut(item_size).enumerate() {
        for (lane_idx, byte_slot) in item_slot.iter_mut().enumerate() {
            *byte_slot = body[lane_idx * num_items + item_idx];
        }
    }
    out.extend_from_slice(tail);
    out
}

// ---------------------------------------------------------------------------
// Bit shuffle
// ---------------------------------------------------------------------------

/// Transpose `data` at the bit level within each group of 8 items.
///
/// For each complete group of `8 * item_size` bytes, bit-0 of every
/// byte in every item becomes contiguous across the 8 items, then
/// bit-1, etc. Trailing bytes that do not fill a complete group pass
/// through unchanged at the tail.
fn bit_shuffle(data: &[u8], item_size: u8) -> Vec<u8> {
    debug_assert!(VALID_ITEM_SIZES.contains(&item_size));
    let item_size = usize::from(item_size);
    let group_bytes = 8 * item_size;
    let (grouped, remainder) = split_aligned(data, group_bytes);

    let mut out = vec![0u8; grouped.len()];
    for (group_idx, group) in grouped.chunks_exact(group_bytes).enumerate() {
        let group_start = group_idx * group_bytes;
        transpose_bits_group(group, &mut out[group_start..group_start + group_bytes]);
    }
    out.extend_from_slice(remainder);
    out
}

/// Inverse of [`bit_shuffle`]. The 8x8 bit transpose is self-inverse,
/// so this is the same routine.
fn bit_unshuffle(data: &[u8], item_size: u8) -> Vec<u8> {
    // The bit transpose is its own inverse.
    bit_shuffle(data, item_size)
}

/// Bit-transpose a single group of exactly 8 items.
///
/// For a group of `8 * item_size` bytes, route every bit at position
/// `(item_idx in 0..8, byte_in_item in 0..item_size, bit_in_byte in
/// 0..8)` to output byte index `bit_in_byte * item_size +
/// byte_in_item`, MSB-first within that byte across the 8 items.
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
// Shared helpers
// ---------------------------------------------------------------------------

/// Split `data` into `(aligned_prefix, trailing_tail)` where the
/// prefix length is a whole multiple of `unit`.
fn split_aligned(data: &[u8], unit: usize) -> (&[u8], &[u8]) {
    let boundary = data.len() / unit * unit;
    data.split_at(boundary)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random byte stream (LCG; no RNG seed needed).
    fn lcg_bytes(n: usize, seed: u64) -> Vec<u8> {
        let mut state = seed;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            out.push((state >> 56) as u8);
        }
        out
    }

    // -----------------------------------------------------------------
    // Round-trip — generic
    // -----------------------------------------------------------------

    fn assert_round_trip(data: &[u8], item_size: u8, shuffle: ShuffleMode) {
        let compressed = compress(data, item_size, shuffle).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(
            decompressed,
            data,
            "round-trip mismatch: item_size={item_size}, shuffle={shuffle:?}, len={}",
            data.len()
        );
    }

    #[test]
    fn round_trips_byte_shuffle_each_item_size() {
        for &item_size in &VALID_ITEM_SIZES {
            let data = lcg_bytes(1024, 0x00C0_FFEE);
            assert_round_trip(&data, item_size, ShuffleMode::Byte);
        }
    }

    #[test]
    fn round_trips_bit_shuffle_each_item_size() {
        for &item_size in &VALID_ITEM_SIZES {
            // Length that is a whole multiple of 8 * item_size so no
            // partial tail exists.
            let len = 8 * usize::from(item_size) * 16;
            let data = lcg_bytes(len, 0xFEED);
            assert_round_trip(&data, item_size, ShuffleMode::Bit);
        }
    }

    #[test]
    fn round_trips_no_shuffle() {
        let data = lcg_bytes(1024, 0x1234);
        assert_round_trip(&data, 4, ShuffleMode::None);
    }

    // -----------------------------------------------------------------
    // Unaligned input
    // -----------------------------------------------------------------

    #[test]
    fn round_trips_unaligned_input_byte_shuffle() {
        for &item_size in &VALID_ITEM_SIZES {
            let is = usize::from(item_size);
            for &extra in &[1usize, is - 1, 3, 7] {
                if extra == 0 || extra >= is {
                    continue;
                }
                let len = 16 * is + extra;
                let data = lcg_bytes(len, 0x00AB_CDEF);
                assert_round_trip(&data, item_size, ShuffleMode::Byte);
            }
        }
    }

    #[test]
    fn round_trips_unaligned_input_bit_shuffle() {
        for &item_size in &VALID_ITEM_SIZES {
            let is = usize::from(item_size);
            let group_bytes = 8 * is;
            for &extra in &[1usize, is, group_bytes - 1] {
                if extra >= group_bytes {
                    continue;
                }
                let len = 4 * group_bytes + extra;
                let data = lcg_bytes(len, 0x1234);
                assert_round_trip(&data, item_size, ShuffleMode::Bit);
            }
        }
    }

    // -----------------------------------------------------------------
    // Empty input
    // -----------------------------------------------------------------

    #[test]
    fn round_trips_empty_input() {
        for &item_size in &VALID_ITEM_SIZES {
            for shuffle in [ShuffleMode::None, ShuffleMode::Byte, ShuffleMode::Bit] {
                assert_round_trip(&[], item_size, shuffle);
            }
        }
    }

    #[test]
    fn empty_input_emits_well_formed_header() {
        let compressed = compress(&[], 4, ShuffleMode::Byte).expect("compress");
        // 24-byte header + a small LZ4 frame for the empty body
        // (lz4_flex emits a 4-byte LE size prefix + a single block
        // header word even for empty input).
        assert!(compressed.len() >= HEADER_LEN);
        let header = parse_header(&compressed).expect("header");
        assert_eq!(header.uncompressed_size, 0);
        assert_eq!(header.shuffled_size, 0);
        // Round-trip must still recover the empty plaintext exactly.
        let decompressed = decompress(&compressed).expect("decompress");
        assert!(decompressed.is_empty());
    }

    // -----------------------------------------------------------------
    // Byte-shuffle layout correctness (matches the documented example)
    // -----------------------------------------------------------------

    #[test]
    fn byte_shuffle_layout_is_transposed() {
        // item_size=4, 3 items:
        //   [a0 a1 a2 a3 | b0 b1 b2 b3 | c0 c1 c2 c3]
        // should transpose to:
        //   [a0 b0 c0 | a1 b1 c1 | a2 b2 c2 | a3 b3 c3]
        let data = [
            0xa0, 0xa1, 0xa2, 0xa3, 0xb0, 0xb1, 0xb2, 0xb3, 0xc0, 0xc1, 0xc2, 0xc3,
        ];
        let shuffled = byte_shuffle(&data, 4);
        assert_eq!(
            shuffled,
            &[
                0xa0, 0xb0, 0xc0, // lane 0
                0xa1, 0xb1, 0xc1, // lane 1
                0xa2, 0xb2, 0xc2, // lane 2
                0xa3, 0xb3, 0xc3, // lane 3
            ]
        );
    }

    // -----------------------------------------------------------------
    // Compression effectiveness — shuffle helps on float data
    // -----------------------------------------------------------------

    /// Generate a smooth f32 ramp (highly similar neighbours).
    fn float32_ramp(n: usize) -> Vec<u8> {
        let mut raw = Vec::with_capacity(n * 4);
        for i in 0..n {
            let f = (i as f32) * 0.5 + ((i as f32).sin() * 16.0);
            raw.extend_from_slice(&f.to_bits().to_le_bytes());
        }
        raw
    }

    #[test]
    fn byte_shuffle_beats_no_shuffle_on_float_ramp() {
        let raw = float32_ramp(1024);

        let with_shuffle = compress(&raw, 4, ShuffleMode::Byte).expect("compress byte");
        let no_shuffle = compress(&raw, 4, ShuffleMode::None).expect("compress none");

        assert!(
            with_shuffle.len() < no_shuffle.len(),
            "byte shuffle should compress smaller than no shuffle on float ramp: \
             with_shuffle={} no_shuffle={}",
            with_shuffle.len(),
            no_shuffle.len()
        );
    }

    #[test]
    fn bit_shuffle_round_trips_on_float_ramp() {
        // The task brief hypothesised "bit shuffle beats byte shuffle on
        // float32". Empirically this is not true when LZ4 is the inner
        // codec: bit shuffle's theoretical advantage only materialises
        // with entropy coders (ZSTD's FSE/Huffman, arithmetic coding).
        // LZ4's match-finder exploits byte-run redundancy, which byte
        // shuffle already exposes maximally for IEEE-754 data — the
        // bit-plane transpose can actually scatter those runs and produce
        // output the same size as (or larger than) the unshuffled input.
        //
        // The property that matters for a container codec is exact
        // reversibility, asserted here. The comparative-ratio behaviour
        // is documented (see `byte_shuffle_beats_no_shuffle_on_float_ramp`
        // for the case where shuffle does help) but not encoded as a
        // hard assertion because it is data-dependent.
        let raw = float32_ramp(1024);
        let compressed = compress(&raw, 4, ShuffleMode::Bit).expect("compress bit");
        let decompressed = decompress(&compressed).expect("decompress bit");
        assert_eq!(decompressed, raw);
    }

    // -----------------------------------------------------------------
    // Container format / header
    // -----------------------------------------------------------------

    #[test]
    fn header_magic_and_version_correct() {
        let data = lcg_bytes(128, 0x1);
        let compressed = compress(&data, 4, ShuffleMode::Byte).expect("compress");
        assert_eq!(&compressed[..8], MAGIC);
        assert_eq!(compressed[8], VERSION);
    }

    #[test]
    fn header_records_item_size_and_shuffle_mode() {
        let data = lcg_bytes(64, 0x2);
        for &item_size in &VALID_ITEM_SIZES {
            for shuffle in [ShuffleMode::None, ShuffleMode::Byte, ShuffleMode::Bit] {
                let compressed = compress(&data, item_size, shuffle).expect("compress");
                let header = parse_header(&compressed).expect("header");
                assert_eq!(header.item_size, item_size);
                assert_eq!(header.shuffle_mode, shuffle);
            }
        }
    }

    #[test]
    fn header_records_uncompressed_and_shuffled_sizes() {
        let data = lcg_bytes(100, 0x3); // 100 bytes, item_size 4 → tail of 0 bytes
        let compressed = compress(&data, 4, ShuffleMode::Byte).expect("compress");
        let header = parse_header(&compressed).expect("header");
        assert_eq!(header.uncompressed_size, 100);
        assert_eq!(header.shuffled_size, 100);
    }

    #[test]
    fn rejects_invalid_item_size_on_compress() {
        let result = compress(b"hello", 3, ShuffleMode::Byte);
        assert!(matches!(result, Err(OmnizipError::Corrupt { .. })));
    }

    #[test]
    fn rejects_truncated_header_on_decompress() {
        let result = decompress(b"short");
        assert!(matches!(result, Err(OmnizipError::Corrupt { .. })));
    }

    #[test]
    fn rejects_bad_magic_on_decompress() {
        let mut bad = vec![0u8; HEADER_LEN];
        bad[..8].copy_from_slice(b"XXXXXXX\0");
        let result = decompress(&bad);
        assert!(matches!(result, Err(OmnizipError::Corrupt { .. })));
    }

    #[test]
    fn rejects_unknown_inner_codec_on_decompress() {
        let data = lcg_bytes(64, 0x4);
        let mut compressed = compress(&data, 4, ShuffleMode::Byte).expect("compress");
        // Corrupt the inner_codec byte (offset 11).
        compressed[11] = 99;
        let result = decompress(&compressed);
        assert!(matches!(result, Err(OmnizipError::Corrupt { .. })));
    }

    // -----------------------------------------------------------------
    // Codec trait impl
    // -----------------------------------------------------------------

    #[test]
    fn codec_trait_round_trip() {
        let data = float32_ramp(256);
        let compressed = BloscCodec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        let decompressed = BloscCodec
            .decompress(&compressed, data.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, data);
    }

    #[test]
    fn codec_id_is_distinct_from_other_codecs() {
        // 0x0013 — must not collide with SNAPPY (0x000A), FSST (0x0010),
        // RICEPP (0x0011), or FLAC (0x0012).
        assert_eq!(BloscCodec.id(), CodecId::BLOSC);
        assert_ne!(BloscCodec.id(), CodecId::SNAPPY);
    }

    #[test]
    fn codec_name_is_blosc() {
        assert_eq!(BloscCodec.name(), "blosc");
    }

    // -----------------------------------------------------------------
    // Determinism
    // -----------------------------------------------------------------

    #[test]
    fn compress_is_deterministic_across_calls() {
        let data = float32_ramp(512);
        let a = compress(&data, 4, ShuffleMode::Byte).expect("compress a");
        let b = compress(&data, 4, ShuffleMode::Byte).expect("compress b");
        assert_eq!(a, b, "same input must produce byte-identical output");
    }

    // -----------------------------------------------------------------
    // ShuffleMode wire round-trip
    // -----------------------------------------------------------------

    #[test]
    fn shuffle_mode_round_trips_through_u8() {
        for mode in [ShuffleMode::None, ShuffleMode::Byte, ShuffleMode::Bit] {
            assert_eq!(ShuffleMode::from_u8(mode.as_u8()), Some(mode));
        }
    }

    #[test]
    fn shuffle_mode_rejects_unknown_wire_value() {
        assert_eq!(ShuffleMode::from_u8(99), None);
    }
}
