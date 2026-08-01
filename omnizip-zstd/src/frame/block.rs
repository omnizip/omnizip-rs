//! Block header parser — ported from
//! `omnizip/lib/omnizip/algorithms/zstandard/frame/block.rb`
//! (126 LOC, MIT, Ribose Inc.).
//!
//! ## Block header layout (RFC 8878 §3.1.1.2)
//!
//! 3 bytes, little-endian. Treated as a 24-bit integer:
//!
//! ```text
//! bit  0       Last_Block flag (1 = last block in frame)
//! bits 1..2    Block_Type (0=Raw, 1=RLE, 2=Compressed, 3=Reserved)
//! bits 3..23   Block_Size (meaning depends on Block_Type)
//! ```
//!
//! For Raw / Compressed, `Block_Size` is the byte count of the block
//! content. For RLE, `Block_Size` is the *output* length (the block
//! content is a single byte that gets repeated).

#![forbid(unsafe_code)]

use crate::constants::{
    BLOCK_HEADER_SIZE, BLOCK_TYPE_COMPRESSED, BLOCK_TYPE_RAW, BLOCK_TYPE_RESERVED, BLOCK_TYPE_RLE,
};
use crate::ZstdError;

/// Parsed block header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockHeader {
    pub last_block: bool,
    pub block_type: u8,
    pub block_size: u32,
    pub raw: u32,
}

impl BlockHeader {
    /// Parse 3 bytes from the head of `input`. Returns the parsed
    /// header and the slice that follows it.
    ///
    /// # Errors
    ///
    /// Returns [`ZstdError::Corrupt`] if `input` is shorter than
    /// `BLOCK_HEADER_SIZE` (3 bytes).
    pub fn parse(input: &[u8]) -> Result<(Self, &[u8]), ZstdError> {
        if input.len() < BLOCK_HEADER_SIZE {
            return Err(ZstdError::Corrupt {
                reason: format!(
                    "block header needs {BLOCK_HEADER_SIZE} bytes, got {}",
                    input.len()
                ),
            });
        }
        let raw = u32::from(input[0])
            | (u32::from(input[1]) << 8)
            | (u32::from(input[2]) << 16);
        Ok((
            Self {
                last_block: (raw & 0x01) != 0,
                block_type: ((raw >> 1) & 0x03) as u8,
                block_size: (raw >> 3) & 0x001F_FFFF,
                raw,
            },
            &input[BLOCK_HEADER_SIZE..],
        ))
    }

    /// Type is `Raw` (uncompressed).
    #[must_use]
    pub const fn is_raw(&self) -> bool {
        self.block_type == BLOCK_TYPE_RAW
    }

    /// Type is `RLE` (single byte repeated).
    #[must_use]
    pub const fn is_rle(&self) -> bool {
        self.block_type == BLOCK_TYPE_RLE
    }

    /// Type is `Compressed`.
    #[must_use]
    pub const fn is_compressed(&self) -> bool {
        self.block_type == BLOCK_TYPE_COMPRESSED
    }

    /// Type is `Reserved` (must reject).
    #[must_use]
    pub const fn is_reserved(&self) -> bool {
        self.block_type == BLOCK_TYPE_RESERVED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extracts_fields_correctly() {
        // raw = 0x0800 = 0b1000_0000_0000 (only bit 11 set)
        //   last_block = bit 0 → 0
        //   block_type = bits 1..2 → 0 (raw)
        //   block_size = bits 3..23 → 0x0800 >> 3 = 256
        let bytes = [0x00u8, 0x08, 0x00];
        let (h, rest) = BlockHeader::parse(&bytes).expect("parse");
        assert!(!h.last_block);
        assert!(h.is_raw());
        assert_eq!(h.block_size, 256);
        assert_eq!(h.raw, 0x0800);
        assert!(rest.is_empty());
    }

    #[test]
    fn parse_handles_last_block_flag() {
        // raw = 1 → last_block=true, type=0, size=0
        let (h, _) = BlockHeader::parse(&[0x01, 0x00, 0x00]).expect("parse");
        assert!(h.last_block);
        assert!(h.is_raw());
        assert_eq!(h.block_size, 0);
    }

    #[test]
    fn parse_handles_each_block_type() {
        // type=1 (RLE): raw = 0b00000010 = 0x02
        let (h_rle, _) = BlockHeader::parse(&[0x02, 0x00, 0x00]).expect("parse");
        assert!(h_rle.is_rle());
        // type=2 (Compressed): raw = 0b00000100 = 0x04
        let (h_cmp, _) = BlockHeader::parse(&[0x04, 0x00, 0x00]).expect("parse");
        assert!(h_cmp.is_compressed());
        // type=3 (Reserved): raw = 0b00000110 = 0x06
        let (h_rsv, _) = BlockHeader::parse(&[0x06, 0x00, 0x00]).expect("parse");
        assert!(h_rsv.is_reserved());
    }

    #[test]
    fn parse_rejects_truncated_header() {
        assert!(BlockHeader::parse(&[0x00, 0x01]).is_err());
    }

    #[test]
    fn parse_extracts_max_block_size() {
        // block_size field is 21 bits wide. Set every size bit:
        // raw = 0x00 | (0xFF << 8) | (0xFF << 16) → top byte's bit 7
        // would be bit 23 of raw, which is outside block_size.
        // bits 3..23 = mask 0x1FFFFF. raw bytes that set all of those:
        // raw = 0xFFFFF8 → block_size = 0xFFFFF8 >> 3 = 0x1FFFFF.
        let bytes = [0xF8, 0xFF, 0xFF];
        let (h, _) = BlockHeader::parse(&bytes).expect("parse");
        assert_eq!(h.block_size, 0x001F_FFFF);
    }
}
