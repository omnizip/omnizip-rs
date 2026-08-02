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
    // C reference (zstd_decompress_block.c): `litEncType = istart[0] & 3`.
    // The block_type lives in bits 0-1 (NOT bits 6-7 as RFC 8878 §3.1.1.3.1
    // ambiguously suggests — the C source is authoritative).
    let block_type = header0 & 0x03;

    match block_type {
        LITERALS_BLOCK_RAW => decode_raw(input),
        LITERALS_BLOCK_RLE => decode_rle(input),
        LITERALS_BLOCK_COMPRESSED => decode_compressed(input, None, false),
        LITERALS_BLOCK_TREELESS => decode_compressed(input, previous_huffman_table, true),
        _ => unreachable!("block_type is masked to 2 bits"),
    }
}

// ── Size-format helpers ─────────────────────────────────────────────────
//
// Per the C reference (zstd_decompress_block.c, `set_basic` and `set_rle`):
// the 2-bit `lhlCode = (byte0 >> 2) & 3` selects the header layout:
//
//   lhlCode 0 | 2 → 1-byte header, litSize = byte0 >> 3
//   lhlCode 1     → 2-byte header, litSize = u16_LE(bytes 0..2) >> 4
//   lhlCode 3     → 3-byte header, litSize = u24_LE(bytes 0..3) >> 4

fn decode_size_format_raw_rle(header0: u8, input: &[u8]) -> Result<(u32, usize), ZstdError> {
    let lhl_code = (header0 >> 2) & 0x03;
    match lhl_code {
        0 | 2 => Ok((u32::from(header0 >> 3), 1)),
        1 => {
            if input.len() < 2 {
                return Err(ZstdError::Corrupt {
                    reason: "truncated 2-byte Raw/RLE literals header".into(),
                });
            }
            let lhc = u16::from_le_bytes([input[0], input[1]]);
            Ok((u32::from(lhc >> 4), 2))
        }
        3 => {
            if input.len() < 3 {
                return Err(ZstdError::Corrupt {
                    reason: "truncated 3-byte Raw/RLE literals header".into(),
                });
            }
            let lhc = u32::from(input[0]) | (u32::from(input[1]) << 8) | (u32::from(input[2]) << 16);
            Ok((lhc >> 4, 3))
        }
        _ => unreachable!("lhl_code is masked to 2 bits"),
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
    input: &'t [u8],
    previous_table: Option<&HuffmanTable>,
    // If `true`, this is a Treeless block (set_repeat) — the previous
    // Huffman table is mandatory, and no table bytes are read from
    // the wire.
    is_repeat: bool,
) -> Result<LiteralsSection<'t>, ZstdError> {
    // Per C reference (zstd_decompress_block.c, case set_compressed):
    //   lhlCode = (byte0 >> 2) & 3
    //   lhc     = LE32(bytes 0..4)  (only the relevant bytes matter per case)
    //   case 0 | 1 → lhSize=3, singleStream=!lhlCode (case 0 → 1, case 1 → 0)
    //                litSize  = (lhc >> 4)  & 0x3FF  (10 bits)
    //                litCSize = (lhc >> 14) & 0x3FF  (10 bits)
    //   case 2     → lhSize=4
    //                litSize  = (lhc >> 4)  & 0x3FFF (14 bits)
    //                litCSize =  lhc >> 18           (14 bits)
    //   case 3     → lhSize=5
    //                litSize  = (lhc >> 4)  & 0x3FFFF (18 bits)
    //                litCSize =  lhc >> 22            (18 bits)
    if input.is_empty() {
        return Err(ZstdError::Corrupt {
            reason: "empty compressed literals header".into(),
        });
    }
    if is_repeat && previous_table.is_none() {
        // C: `RETURN_ERROR_IF(dctx->litEntropy==0, dictionary_corrupted, "")`.
        return Err(ZstdError::Corrupt {
            reason: "treeless literals block requires a prior compressed block in the same frame".into(),
        });
    }
    let header0 = input[0];
    let lhl_code = (header0 >> 2) & 0x03;

    let (lit_size, lit_c_size, lh_size, single_stream): (u32, u32, usize, bool) = match lhl_code {
        0 | 1 => {
            if input.len() < 3 {
                return Err(ZstdError::Corrupt {
                    reason: "truncated 3-byte compressed literals header".into(),
                });
            }
            let lhc = u32::from_le_bytes([input[0], input[1], input[2], 0]);
            (
                (lhc >> 4) & 0x3FF,
                (lhc >> 14) & 0x3FF,
                3,
                lhl_code == 0,
            )
        }
        2 => {
            if input.len() < 4 {
                return Err(ZstdError::Corrupt {
                    reason: "truncated 4-byte compressed literals header".into(),
                });
            }
            let lhc = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);
            ((lhc >> 4) & 0x3FFF, lhc >> 18, 4, false)
        }
        3 => {
            if input.len() < 5 {
                return Err(ZstdError::Corrupt {
                    reason: "truncated 5-byte compressed literals header".into(),
                });
            }
            // 5-byte header: bits 4..21 = litSize (18 bits),
            // bits 22..39 = litCSize (18 bits). Read as two separate LE
            // values to avoid u64 shift overhead.
            let low = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);
            let high = u32::from(input[4]);
            let lhc = u64::from(low) | (u64::from(high) << 32);
            ((lhc >> 4) as u32 & 0x3FFFF, (lhc >> 22) as u32, 5, false)
        }
        _ => unreachable!("lhl_code is masked to 2 bits"),
    };

    let lit_size_us = lit_size as usize;
    let lit_c_size_us = lit_c_size as usize;
    let needed = lh_size.checked_add(lit_c_size_us).ok_or_else(|| ZstdError::Corrupt {
        reason: format!("compressed literals size {lit_c_size} overflows usize"),
    })?;
    if input.len() < needed {
        return Err(ZstdError::Corrupt {
            reason: format!(
                "truncated compressed literals: need {needed} bytes, got {}",
                input.len()
            ),
        });
    }
    let compressed = &input[lh_size..needed];

    // Read the Huffman table (or reuse the previous one for Treeless).
    // C reference: set_compressed reads a new table; set_repeat (=Treeless)
    // reuses dctx->HUFptr.
    let local_table;
    let table: &HuffmanTable = if is_repeat {
        // Treeless: previous_table must be Some (checked above).
        previous_table.expect("treeless block without previous table")
    } else {
        let (t, _) = crate::huffman::weights::read_huffman_table(compressed)?;
        local_table = t;
        &local_table
    };

    // The Huffman table bytes are part of `compressed`. The C reference
    // returns iSize+1 from HUF_readStats (1 header byte + iSize). Then
    // the literal bitstream starts after that.
    let table_bytes = if is_repeat {
        0
    } else {
        // Read again just to get the consumed count.
        crate::huffman::weights::read_huffman_table(compressed)?.1
    };

    let literal_data = &compressed[table_bytes..];
    let mut literals = vec![0u8; lit_size_us];
    if single_stream {
        decode_single_stream(table, literal_data, &mut literals)?;
    } else {
        decode_four_stream(table, literal_data, &mut literals)?;
    }

    Ok(LiteralsSection {
        literals,
        huffman_table: if is_repeat {
            None
        } else {
            Some(table.clone())
        },
        consumed: needed,
        _phantom: std::marker::PhantomData,
    })
}

/// Single-stream Huffman decode: one forward bitstream decoded into all
/// `lit_size` symbols. Used when `lhl_code == 0` (singleStream=1).
fn decode_single_stream(
    table: &HuffmanTable,
    src: &[u8],
    out: &mut [u8],
) -> Result<(), ZstdError> {
    let mut dec = crate::huffman::HuffmanDecoder::new(table, src);
    dec.decode_into(out)
}

/// Four-stream Huffman decode: the input is split into 6 bytes of
/// jump-table + 4 reverse (`BIT_DStream`) bitstreams. Each stream
/// decodes ~1/4 of the literals. Used when `lhl_code != 0`
/// (singleStream=0).
///
/// Layout (C reference `HUF_decompress4X1_usingDTable_internal_body`):
/// ```text
/// bytes 0..6   three little-endian u16 sizes for streams 1, 2, 3
///              (stream 4 size = total - 6 - size1 - size2 - size3)
/// bytes 6..end four streams, concatenated in order 1, 2, 3, 4
/// ```
///
/// Each stream is read backwards (last byte first, MSB-first within
/// each byte) using `BitStream`, matching the C reference's
/// `BIT_initDStream` per stream.
fn decode_four_stream(
    table: &HuffmanTable,
    src: &[u8],
    out: &mut [u8],
) -> Result<(), ZstdError> {
    use crate::fse::BitStream;
    if src.len() < 10 {
        return Err(ZstdError::Corrupt {
            reason: "4-stream huffman literals too short (need ≥10 bytes)".into(),
        });
    }
    let length1 = u16::from_le_bytes([src[0], src[1]]) as usize;
    let length2 = u16::from_le_bytes([src[2], src[3]]) as usize;
    let length3 = u16::from_le_bytes([src[4], src[5]]) as usize;
    let total_stream_bytes = src.len() - 6;
    if length1 + length2 + length3 > total_stream_bytes {
        return Err(ZstdError::Corrupt {
            reason: "4-stream sizes exceed total".into(),
        });
    }
    let length4 = total_stream_bytes - length1 - length2 - length3;
    let _ = length4; // used implicitly by seg4 slicing

    let streams_data = &src[6..];
    // C reference layout: stream 1 starts at istart+6, stream 2 at
    // istart+6+length1, etc. Stream 4 is the last.
    let seg1 = &streams_data[..length1];
    let seg2 = &streams_data[length1..length1 + length2];
    let seg3 = &streams_data[length1 + length2..length1 + length2 + length3];
    let seg4 = &streams_data[length1 + length2 + length3..];

    let mut bs1 = BitStream::new(seg1);
    let mut bs2 = BitStream::new(seg2);
    let mut bs3 = BitStream::new(seg3);
    let mut bs4 = BitStream::new(seg4);

    // C reference: segmentSize = (dstSize+3)/4.
    let segment_size = (out.len() + 3) / 4;
    let mut boundaries = [0usize; 5];
    for i in 1..4 {
        boundaries[i] = segment_size * i;
    }
    boundaries[4] = out.len();

    // Decode each stream into its segment, reloading between symbols.
    let mut bitstreams: [&mut BitStream<'_>; 4] = [&mut bs1, &mut bs2, &mut bs3, &mut bs4];
    for (i, bs) in bitstreams.iter_mut().enumerate() {
        let start = boundaries[i];
        let end = boundaries[i + 1];
        for j in start..end {
            out[j] = table.decode(bs)?;
            bs.reload();
        }
    }
    Ok(())
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
        // block_type = Raw (0) in bits 0-1, lhlCode = 0 in bits 2-3.
        // litSize = byte0 >> 3. For litSize=1: byte0 = 0b00001000 = 0x08.
        let input = [0x08, b'A'];
        let s = decode_literals_section(&input, None).expect("decode");
        assert_eq!(s.literals, b"A");
        assert_eq!(s.consumed, 2);
        assert!(s.huffman_table.is_none());
    }

    #[test]
    fn raw_block_one_byte_header_lhlcode_2() {
        // lhlCode = 2 (bits 2-3 = 10) is also a 1-byte header.
        // byte0 = 0b00001010 = 0x0A → litSize = 0x0A >> 3 = 1.
        let (size, hdr) = decode_size_format_raw_rle(0x0A, &[0x0A]).unwrap();
        assert_eq!(size, 1);
        assert_eq!(hdr, 1);
    }

    #[test]
    fn raw_block_two_byte_header_decodes() {
        // 2-byte header (lhlCode = 1, bits 2-3 = 01 → byte0 & 0x0C = 0x04).
        // lhc = bytes 0-1 LE. litSize = lhc >> 4.
        // For litSize = 0x100: lhc = 0x100 << 4 = 0x1000. byte0 = 0x04, byte1 = 0x10.
        let header0 = LITERALS_BLOCK_RAW | 0x04; // Raw | lhlCode=1
        let header1 = 0x10;
        let (size, hdr) = decode_size_format_raw_rle(header0, &[header0, header1]).unwrap();
        let lhc = u16::from_le_bytes([header0, header1]);
        assert_eq!(size, u32::from(lhc >> 4));
        assert_eq!(hdr, 2);
    }

    #[test]
    fn raw_block_three_byte_header_decodes() {
        // 3-byte header (lhlCode = 3, bits 2-3 = 11 → byte0 & 0x0C = 0x0C).
        // lhc = bytes 0-2 LE (24-bit). litSize = lhc >> 4.
        // For litSize = 0x100: lhc >> 4 = 0x100 → lhc = 0x1000.
        //   byte0 = 0x0C, byte1 = 0x10 → lhc = 0x0C | (0x10 << 8) = 0x100C → >> 4 = 0x100.
        let header0 = LITERALS_BLOCK_RAW | 0x0C;
        let (size, hdr) = decode_size_format_raw_rle(header0, &[header0, 0x10, 0x00]).unwrap();
        assert_eq!(size, 0x100);
        assert_eq!(hdr, 3);
    }

    #[test]
    fn truncated_header_is_corrupt() {
        // 2-byte header indicated (lhlCode=1), only 1 byte supplied.
        let header0 = LITERALS_BLOCK_RAW | 0x04;
        assert!(decode_size_format_raw_rle(header0, &[header0]).is_err());
        // 3-byte header indicated (lhlCode=3), only 2 bytes supplied.
        let header0b = LITERALS_BLOCK_RAW | 0x0C;
        assert!(decode_size_format_raw_rle(header0b, &[header0b, 0x00]).is_err());
    }

    #[test]
    fn rle_block_two_byte_header_decodes() {
        // block_type = RLE (1) in bits 0-1, lhlCode = 1 in bits 2-3.
        // byte0 = 0b00000101 = 0x05. lhc = 0x05 | (0 << 8) = 0x05.
        // litSize = 0x05 >> 4 = 0. Use byte1 = 0x40 → lhc = 0x4005, litSize = 0x400 = 1024.
        // Use byte1=0x00 for litSize=0; use a non-trivial case instead:
        // For litSize = 4: lhc >> 4 = 4 → lhc = 0x40. byte0=0x05, byte1 must give 0x40 from lhc=byte0|byte1<<8.
        //   0x40 - 0x05 = 0x3B; (0x3B + 0x100) & 0xFF00 → need byte1 << 8 = 0x40 - 0x05 wrap.
        // Easier: pick byte0=0x05, byte1=0x04 → lhc=0x0405, litSize = 0x0405 >> 4 = 0x40 = 64.
        let header0 = LITERALS_BLOCK_RLE | 0x04;
        let header1 = 0x04;
        let input = [header0, header1, b'X'];
        let s = decode_literals_section(&input, None).expect("decode");
        let lhc = u16::from_le_bytes([header0, header1]);
        assert_eq!(s.literals, vec![b'X'; u32::from(lhc >> 4) as usize]);
    }

    #[test]
    fn compressed_block_with_insufficient_bytes_is_corrupt() {
        // block_type = Compressed (2) in bits 0-1, lhlCode = 0 → 3-byte header.
        // Need 3 header bytes + at least 1 byte of Huffman stream.
        // With only 5 bytes total the header parses but the Huffman
        // section is too short.
        let input = [LITERALS_BLOCK_COMPRESSED, 0, 0, 0, 0];
        let result = decode_literals_section(&input, None);
        assert!(matches!(result, Err(ZstdError::Corrupt { .. } | ZstdError::Unsupported { .. })));
    }

    #[test]
    fn treeless_block_without_previous_table_is_corrupt() {
        let input = [LITERALS_BLOCK_TREELESS, 0, 0, 0, 0];
        let result = decode_literals_section(&input, None);
        // C reference: set_repeat requires litEntropy; without a prior
        // compressed block in the frame, the decoder rejects it.
        assert!(matches!(result, Err(ZstdError::Corrupt { .. })));
    }
}
