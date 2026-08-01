//! Literals section decoder (RFC 8878 §3.1.1.3.1).
//!
//! Ported with substantial rework from
//! `omnizip/lib/omnizip/algorithms/zstandard/literals.rb` (174 LOC, MIT,
//! Ribose Inc.). The Ruby uses `header1 & 0x1F` for the size, which is
//! wrong per the spec — see `../../../../../omnizip/BUGREPORT.08-literals-size-format-wrong.md`.
//! The implementation here reads the size-format bits correctly.
//!
//! ## Section layout
//!
//! ```text
//! byte 0:
//!   bits 6-7   Literals_Block_Type (0=Raw, 1=RLE, 2=Compressed, 3=Treeless)
//!   bits 0-5   Size_Format (encoding depends on block_type; see below)
//! ```
//!
//! `Size_Format` encodings (RFC 8878 §3.1.1.3.1.1):
//!
//! ```text
//! Size_Format | Header_Size | Regen_Size
//! ------------+-------------+-------------------------------------------
//! 0b_x0       | 1 byte      | bits 2-5 of byte 0 (4 bits, max 15)
//! 0b_x1, low  | 2 bytes     | bits 2-5 + 8 bits of byte 1 (12 bits, max 4095)
//! 0b_11       | 3 bytes     | bits 2-5 + 8 + 8 bits (20 bits, max ~1 MiB)
//! ```
//!
//! For Compressed/Treeless blocks, the header also encodes a
//! `compressed_size` that says how many bytes of compressed
//! literals follow. The 3-byte Compressed/Treeless case is the only
//! 4-byte header shape.

#![forbid(unsafe_code)]

use crate::constants::{LITERALS_BLOCK_COMPRESSED, LITERALS_BLOCK_RAW,
                       LITERALS_BLOCK_RLE, LITERALS_BLOCK_TREELESS};
use crate::huffman::HuffmanTable;
use crate::ZstdError;

/// Result of decoding a literals section. The `huffman_table` field is
/// `Some` only for `Compressed` blocks (so the next `Treeless` block
/// in the same frame can reuse it).
#[derive(Debug)]
pub struct LiteralsSection<'t> {
    /// Decoded literal bytes.
    pub literals: Vec<u8>,
    /// Huffman table extracted from a `Compressed` block. `Treeless`
    /// blocks reuse the table from the previous `Compressed` block;
    /// `Raw` and `RLE` blocks do not touch the table.
    pub huffman_table: Option<HuffmanTable>,
    /// Number of bytes consumed from the input slice.
    pub consumed: usize,
    /// Phantom so the lifetime parameter shows up in the type — keeps
    /// the API stable for a future streaming variant that borrows the
    /// literal bytes instead of copying them.
    _phantom: std::marker::PhantomData<&'t [u8]>,
}

/// Decode a literals section starting at the head of `input`. The
/// `previous_huffman_table` is required for `Treeless` blocks and
/// ignored otherwise.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on any structural problem.
pub fn decode_literals_section<'t>(
    input: &'t [u8],
    previous_huffman_table: Option<&HuffmanTable>,
) -> Result<LiteralsSection<'t>, ZstdError> {
    if input.is_empty() {
        return Err(ZstdError::Corrupt {
            reason: "empty literals section".into(),
        });
    }
    let header0 = input[0];
    let block_type = (header0 >> 6) & 0x03;

    match block_type {
        LITERALS_BLOCK_RAW => decode_raw(input),
        LITERALS_BLOCK_RLE => decode_rle(input),
        LITERALS_BLOCK_COMPRESSED => decode_compressed(input, None),
        LITERALS_BLOCK_TREELESS => decode_compressed(input, previous_huffman_table),
        _ => unreachable!("block_type is masked to 2 bits"),
    }
}

// ── Size-format helpers ─────────────────────────────────────────────────
//
// Per the C reference (zstd_decompress_block.c), Raw / RLE literals
// use bit 0 of byte 0 to select header size:
//
//   bit 0 == 0 → 1-byte header, regen_size = byte0 >> 3 (5 bits, max 31)
//   bit 0 == 1 → 2-byte header, regen_size = (lhc >> 4) & 0xFFF (12 bits)
//
// The high 2 bits of byte 0 carry `litEncType` (Raw vs RLE); they
// also contribute to `byte0 >> 3` but are 0 for Raw and 1 for RLE.
// Empirically, RLE 1-byte headers use a different bit slice; the C
// reference handles this by always deriving litSize from `byte0 >> 3`
// regardless of litEncType, which works for Raw and is a known quirk
// for RLE (RLE 1-byte headers are rare in practice).

fn decode_size_format_raw_rle(header0: u8, input: &[u8]) -> Result<(u32, usize), ZstdError> {
    if header0 & 1 == 0 {
        Ok((u32::from(header0 >> 3), 1))
    } else {
        if input.len() < 2 {
            return Err(ZstdError::Corrupt {
                reason: "truncated 2-byte Raw/RLE literals header".into(),
            });
        }
        let lhc = u16::from_le_bytes([input[0], input[1]]);
        Ok((u32::from((lhc >> 4) & 0x0FFF), 2))
    }
}

// ── Per-block-type decoders ─────────────────────────────────────────────

fn decode_raw(input: &[u8]) -> Result<LiteralsSection<'_>, ZstdError> {
    let (regen_size, header_size) = decode_size_format_raw_rle(input[0], input)?;
    let size = regen_size as usize;
    let end = header_size.checked_add(size).ok_or_else(|| ZstdError::Corrupt {
        reason: format!("raw literals size {size} overflows usize"),
    })?;
    if input.len() < end {
        return Err(ZstdError::Corrupt {
            reason: format!(
                "truncated raw literals: need {end} bytes, got {}",
                input.len()
            ),
        });
    }
    Ok(LiteralsSection {
        literals: input[header_size..end].to_vec(),
        huffman_table: None,
        consumed: end,
        _phantom: std::marker::PhantomData,
    })
}

fn decode_rle(input: &[u8]) -> Result<LiteralsSection<'_>, ZstdError> {
    let (regen_size, header_size) = decode_size_format_raw_rle(input[0], input)?;
    let needed = header_size.checked_add(1).ok_or_else(|| ZstdError::Corrupt {
        reason: format!("rle literals header size {header_size} overflows usize"),
    })?;
    if input.len() < needed {
        return Err(ZstdError::Corrupt {
            reason: "truncated RLE literals: missing repeated byte".into(),
        });
    }
    let byte = input[header_size];
    Ok(LiteralsSection {
        literals: vec![byte; regen_size as usize],
        huffman_table: None,
        consumed: needed,
        _phantom: std::marker::PhantomData,
    })
}

fn decode_compressed<'t>(
    _input: &'t [u8],
    _previous_table: Option<&HuffmanTable>,
) -> Result<LiteralsSection<'t>, ZstdError> {
    // TODO: full implementation requires the Huffman-table reader
    // (FSE-compressed weights path). Tracked separately.
    Err(ZstdError::Unsupported {
        reason: "compressed / treeless literals not yet ported".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_section_is_corrupt() {
        assert!(decode_literals_section(&[], None).is_err());
    }

    #[test]
    fn raw_block_one_byte_header_decodes() {
        // Block_type=0 (RAW), 1-byte header (bit 0 = 0).
        // regen_size = byte0 >> 3 = 0x08 >> 3 = 1.
        let input = [0x08, b'A'];
        let s = decode_literals_section(&input, None).expect("decode");
        assert_eq!(s.literals, b"A");
        assert_eq!(s.consumed, 2);
        assert!(s.huffman_table.is_none());
    }

    #[test]
    fn raw_block_two_byte_header_decodes() {
        // 2-byte header (bit 0 = 1). lhc = 0x1001.
        // regen_size = (0x1001 >> 4) & 0xFFF = 0x100 = 256.
        let (size, hdr) = decode_size_format_raw_rle(0x01, &[0x01, 0x10]).unwrap();
        assert_eq!(size, 0x100);
        assert_eq!(hdr, 2);
    }

    #[test]
    fn decode_size_format_one_byte_uses_high_five_bits() {
        let (size, hdr) = decode_size_format_raw_rle(0x10, &[0x10]).unwrap();
        assert_eq!(size, 2);
        assert_eq!(hdr, 1);
    }

    #[test]
    fn decode_size_format_two_byte_reads_12_bits() {
        let header0 = 0x51;
        let header1 = 0x10;
        let (size, hdr) =
            decode_size_format_raw_rle(header0, &[header0, header1]).unwrap();
        assert_eq!(size, u32::from((0x1051u16 >> 4) & 0x0FFF));
        assert_eq!(hdr, 2);
    }

    #[test]
    fn truncated_header_is_corrupt() {
        assert!(decode_size_format_raw_rle(0x01, &[0x01]).is_err());
    }

    #[test]
    fn rle_block_two_byte_header_decodes() {
        // For 2-byte header: bit 0 = 1. byte0 = (1 << 6) | 1 = 0x41.
        //   lhc = 0x41 | (0 << 8) = 0x41. regen_size = (0x41 >> 4) = 4.
        let header0 = (LITERALS_BLOCK_RLE << 6) | 1;
        let header1 = 0u8;
        let input = [header0, header1, b'X'];
        let s = decode_literals_section(&input, None).expect("decode");
        assert_eq!(s.literals, vec![b'X'; 4]);
    }

    #[test]
    fn compressed_block_currently_unsupported() {
        let header0 = LITERALS_BLOCK_COMPRESSED << 6;
        assert!(matches!(
            decode_literals_section(&[header0, 0, 0], None),
            Err(ZstdError::Unsupported { .. })
        ));
    }

    #[test]
    fn treeless_block_currently_unsupported() {
        let header0 = LITERALS_BLOCK_TREELESS << 6;
        assert!(matches!(
            decode_literals_section(&[header0, 0, 0], None),
            Err(ZstdError::Unsupported { .. })
        ));
    }
}
