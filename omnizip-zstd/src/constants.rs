//! Zstandard format constants — ported from
//! `omnizip/lib/omnizip/algorithms/zstandard/constants.rb` (141 LOC).
//!
//! These are the values pinned by RFC 8878 §3–4. They define the frame
//! magic, block types, FSE parameters, and compression-level ranges
//! that every ZSTD implementation must agree on.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

// ── Frame magic ──────────────────────────────────────────────────────────

/// ZSTD frame magic number (little-endian u32 = `0xFD2FB528`).
pub const MAGIC_NUMBER: u32 = 0xFD2F_B528;

/// Magic bytes as they appear on the wire (byte 0 first).
pub const MAGIC_BYTES: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Base for skippable-frame magic (`0x184D2A50`..`0x184D2A5F`).
pub const SKIPPABLE_MAGIC_BASE: u32 = 0x184D_2A50;

/// Mask to test for skippable-frame magic: `frame & MASK == BASE`.
pub const SKIPPABLE_MAGIC_MASK: u32 = 0xFFFF_FFF0;

// ── Block types ──────────────────────────────────────────────────────────

/// Raw block: uncompressed data follows directly.
pub const BLOCK_TYPE_RAW: u8 = 0;

/// RLE block: single byte repeated `Block_Size` times.
pub const BLOCK_TYPE_RLE: u8 = 1;

/// Compressed block: contains literals + sequences sections.
pub const BLOCK_TYPE_COMPRESSED: u8 = 2;

/// Reserved block type. Decoder MUST reject.
pub const BLOCK_TYPE_RESERVED: u8 = 3;

/// Block header is 3 bytes (LE).
pub const BLOCK_HEADER_SIZE: usize = 3;

/// Maximum compressed block size (128 KiB).
pub const BLOCK_MAX_SIZE: usize = 128 * 1024;

// ── Literals block types ─────────────────────────────────────────────────

pub const LITERALS_BLOCK_RAW: u8 = 0;
pub const LITERALS_BLOCK_RLE: u8 = 1;
pub const LITERALS_BLOCK_COMPRESSED: u8 = 2;
pub const LITERALS_BLOCK_TREELESS: u8 = 3;

/// Maximum Huffman code length for literal compression.
pub const HUFFMAN_MAX_BITS: u8 = 11;

// ── Sequence compression modes ───────────────────────────────────────────

pub const MODE_PREDEFINED: u8 = 0;
pub const MODE_RLE: u8 = 1;
pub const MODE_FSE: u8 = 2;
pub const MODE_REPEAT: u8 = 3;

// ── FSE accuracy logs ────────────────────────────────────────────────────

/// FSE accuracy log for literal-length codes.
pub const LITERALS_LENGTH_ACCURACY_LOG: u8 = 6;

/// FSE accuracy log for match-length codes.
pub const MATCH_LENGTH_ACCURACY_LOG: u8 = 6;

/// FSE accuracy log for offset codes.
pub const OFFSET_ACCURACY_LOG: u8 = 5;

/// Maximum FSE accuracy log (RFC 8878 §4.1).
pub const FSE_MAX_ACCURACY_LOG: u8 = 9;

/// Minimum FSE accuracy log.
pub const FSE_MIN_ACCURACY_LOG: u8 = 5;

// ── Repeat offsets ───────────────────────────────────────────────────────

pub const REPEAT_OFFSET_1: u8 = 1;
pub const REPEAT_OFFSET_2: u8 = 2;
pub const REPEAT_OFFSET_3: u8 = 3;

/// Default repeat-offset values for a fresh frame.
pub const DEFAULT_REPEAT_OFFSETS: [u32; 3] = [1, 4, 8];

// ── Window ───────────────────────────────────────────────────────────────

pub const WINDOW_LOG_MIN: u8 = 10;
pub const WINDOW_LOG_MAX: u8 = 41;

// ── Huffman ──────────────────────────────────────────────────────────────

pub const HUFFMAN_MAX_LOG: u8 = 11;
pub const HUFFMAN_MAX_CODE_LENGTH: u8 = 11;
pub const HUFFMAN_STANDARD_TABLE_SIZE: usize = 256;

// ── Compression levels ───────────────────────────────────────────────────

pub const MIN_LEVEL: u8 = 1;
pub const MAX_LEVEL: u8 = 22;
pub const DEFAULT_LEVEL: u8 = 3;

/// Default streaming buffer size (128 KiB).
pub const BUFFER_SIZE: usize = 128 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_bytes_match_u32() {
        let from_bytes = u32::from_le_bytes(MAGIC_BYTES);
        assert_eq!(from_bytes, MAGIC_NUMBER);
    }

    #[test]
    fn skippable_mask_covers_range() {
        for i in 0..=15u32 {
            let frame = SKIPPABLE_MAGIC_BASE + i;
            assert_eq!(
                frame & SKIPPABLE_MAGIC_MASK,
                SKIPPABLE_MAGIC_BASE,
                "skippable frame 0x{frame:08X} should match mask"
            );
        }
    }

    #[test]
    fn block_types_are_distinct() {
        let types = [
            BLOCK_TYPE_RAW,
            BLOCK_TYPE_RLE,
            BLOCK_TYPE_COMPRESSED,
            BLOCK_TYPE_RESERVED,
        ];
        let unique: std::collections::HashSet<u8> = types.iter().copied().collect();
        assert_eq!(unique.len(), 4, "block types must be distinct");
    }

    #[test]
    fn level_range_matches_rfc() {
        const _: () = {
            assert!(DEFAULT_LEVEL >= MIN_LEVEL);
            assert!(DEFAULT_LEVEL <= MAX_LEVEL);
        };
        assert_eq!(MIN_LEVEL, 1);
        assert_eq!(MAX_LEVEL, 22);
    }

    #[test]
    fn window_log_range() {
        const _: () = {
            assert!(WINDOW_LOG_MIN <= WINDOW_LOG_MAX);
        };
        assert_eq!(WINDOW_LOG_MIN, 10);
    }

    #[test]
    fn fse_accuracy_log_range() {
        const _: () = {
            assert!(FSE_MIN_ACCURACY_LOG <= FSE_MAX_ACCURACY_LOG);
        };
        assert_eq!(FSE_MAX_ACCURACY_LOG, 9);
    }

    #[test]
    fn default_repeat_offsets_are_canonical() {
        assert_eq!(DEFAULT_REPEAT_OFFSETS, [1, 4, 8]);
    }
}
