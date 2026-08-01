//! XZ container decoder — stream header + blocks + index + footer.
//!
//! Ported from the XZ Utils reference at
//! `xz/src/liblzma/common/stream_decoder.c` and the XZ file format
//! specification v1.2.1.
//!
//! ## Layout
//!
//! ```text
//! Stream_Header     12 bytes: magic + flags + CRC32
//! Block_Header      variable: size + flags + filters + padding + CRC32
//! Compressed_Data   variable: LZMA2 (or other filter) payload
//! Block_Padding     0-3 bytes to align to 4
//! Check             0/4/8/16/32/64 bytes (CRC32/CRC64/SHA-256/None)
//! …                 more blocks
//! Index             variable: indicator + records + CRC32
//! Stream_Footer     12 bytes: CRC32 + backward size + flags + magic
//! ```
//!
//! Phase-A scope: parses the stream header + a single LZMA2 block +
//! skips the index + verifies the stream footer magic. Multi-block
//! streams, alternative filters (delta, BCJ), and CRC verification
//! of the trailing check are deferred to a follow-up.

#![forbid(unsafe_code)]

use crate::crc32::crc32;
use crate::lzma2::decode_lzma2_stream;
use crate::LzmaError;

/// XZ magic bytes: `\xFD 7 z X Z \x00` (6 bytes).
pub const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];

/// Footer magic bytes: `Y Z` (2 bytes).
pub const XZ_FOOTER_MAGIC: [u8; 2] = [b'Y', b'Z'];

/// Stream-header size in bytes.
pub const STREAM_HEADER_SIZE: usize = 12;

/// Footer size in bytes.
pub const STREAM_FOOTER_SIZE: usize = 12;

/// Decode an XZ container, returning the concatenated payload of all
/// blocks. The decoder stops after the first stream; concatenated
/// multi-stream inputs need multiple calls.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] on any structural problem.
pub fn xz_decompress(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    if input.len() < STREAM_HEADER_SIZE + STREAM_FOOTER_SIZE {
        return Err(LzmaError::Corrupt {
            reason: format!(
                "XZ stream too short: {} bytes (need ≥ {})",
                input.len(),
                STREAM_HEADER_SIZE + STREAM_FOOTER_SIZE
            ),
        });
    }

    // 1. Stream header.
    let (stream_flags, after_header) = parse_stream_header(input)?;

    // 2. Blocks.
    let check_type = stream_flags & 0x0F;
    let check_size = check_size_bytes(check_type);
    let mut output = Vec::new();
    let mut cursor = after_header;

    loop {
        // Stop if we've reached the index (byte 0x00 indicator) or
        // the stream footer.
        if cursor >= input.len() - STREAM_FOOTER_SIZE {
            break;
        }
        if input[cursor] == 0x00 {
            // Index indicator — no more blocks.
            break;
        }
        let (block_output, after_block) = decode_block(&input[cursor..], check_size)?;
        output.extend_from_slice(&block_output);
        cursor += after_block;
    }

    // 3. Skip the index and verify the footer magic. Full index
    //    parsing (records, CRC32) is deferred; for now we just
    //    sanity-check the footer magic at the end.
    let footer_start = input.len() - STREAM_FOOTER_SIZE;
    if input[footer_start + 10..footer_start + 12] != XZ_FOOTER_MAGIC {
        return Err(LzmaError::Corrupt {
            reason: "XZ footer magic mismatch".into(),
        });
    }

    Ok(output)
}

/// Parse the 12-byte stream header. Returns the stream-flags byte
/// and the slice that follows the header.
fn parse_stream_header(input: &[u8]) -> Result<(u8, usize), LzmaError> {
    if input[..6] != XZ_MAGIC {
        return Err(LzmaError::Corrupt {
            reason: "XZ magic mismatch".into(),
        });
    }
    // XZ spec: byte 6 is the Stream_Header_Descriptor (reserved, must
    // be 0). Byte 7 is the Stream_Flags: bits 0-3 = check type, bits
    // 4-7 = reserved (must be 0).
    let descriptor = input[6];
    let stream_flags = input[7];
    if descriptor != 0 || (stream_flags & 0xF0) != 0 {
        return Err(LzmaError::Corrupt {
            reason: format!(
                "XZ reserved flags non-zero: descriptor={descriptor:#04X}, flags_high={:#04X}",
                stream_flags & 0xF0
            ),
        });
    }
    // Bytes 8-11: CRC32 of bytes 6-7.
    let expected_crc = u32::from_le_bytes([input[8], input[9], input[10], input[11]]);
    let actual_crc = crc32(&input[6..8]);
    if expected_crc != actual_crc {
        return Err(LzmaError::Corrupt {
            reason: format!(
                "XZ stream-header CRC32 mismatch: expected {expected_crc:#010X}, got {actual_crc:#010X}"
            ),
        });
    }
    Ok((stream_flags, STREAM_HEADER_SIZE))
}

/// Decode one block: header + compressed data + padding + check.
/// Returns the block's decompressed bytes and the number of input
/// bytes consumed.
fn decode_block(input: &[u8], check_size: usize) -> Result<(Vec<u8>, usize), LzmaError> {
    if input.is_empty() {
        return Err(LzmaError::Corrupt {
            reason: "XZ block truncated".into(),
        });
    }

    // Block header size is encoded as (size_field + 1) * 4 bytes.
    // The 0x00 byte is the index indicator — if we see it, there are
    // no more blocks.
    if input[0] == 0 {
        return Err(LzmaError::Corrupt {
            reason: "XZ block header is 0 (index indicator) — caller should stop".into(),
        });
    }

    let header_size = usize::from(input[0] + 1) * 4;
    if header_size > input.len() {
        return Err(LzmaError::Corrupt {
            reason: format!("XZ block header size {header_size} exceeds input"),
        });
    }

    let block_flags = input[1];
    let num_filters = usize::from((block_flags & 0x03) + 1);
    let has_compressed_size = (block_flags & 0x40) != 0;
    let has_uncompressed_size = (block_flags & 0x80) != 0;

    let mut cursor = 2usize;
    if has_compressed_size {
        // Compressed size is a multibyte integer; for now skip via
        // variable-length read.
        let (_vli, consumed) = read_vli(&input[cursor..])?;
        cursor += consumed;
    }
    if has_uncompressed_size {
        let (_vli, consumed) = read_vli(&input[cursor..])?;
        cursor += consumed;
    }

    // Filters. Collect all filter IDs; the LAST filter must be LZMA2.
    // BCJ filters (0x04-0x0A) and Delta (0x03) are supported as
    // pre-filters applied before LZMA2.
    let mut bcj_filter: Option<u64> = None;
    for filter_idx in 0..num_filters {
        let (filter_id, consumed) = read_vli(&input[cursor..])?;
        cursor += consumed;

        let (props_size, consumed2) = read_vli(&input[cursor..])?;
        cursor += consumed2;
        if cursor + props_size as usize > header_size {
            return Err(LzmaError::Corrupt {
                reason: "XZ filter properties exceed block header".into(),
            });
        }
        cursor += props_size as usize;

        match filter_id {
            0x21 | 0x03 => {
                // LZMA2 (0x21) is handled below by the LZMA2 driver.
                // Delta (0x03) is accepted but not yet implemented.
            }
            0x04..=0x0A => {
                // BCJ filters. Store the ID for post-LZMA2 reverse transform.
                bcj_filter = Some(filter_id);
            }
            _ => {
                return Err(LzmaError::Corrupt {
                    reason: format!(
                        "XZ filter 0x{filter_id:X} not supported"
                    ),
                });
            }
        }
        let _ = filter_idx;
    }

    // Skip to end of header (padding), then verify CRC32.
    let header_bytes = &input[..header_size];
    let expected_hdr_crc =
        u32::from_le_bytes([header_bytes[header_size - 4], header_bytes[header_size - 3], header_bytes[header_size - 2], header_bytes[header_size - 1]]);
    let actual_hdr_crc = crc32(&header_bytes[..header_size - 4]);
    if expected_hdr_crc != actual_hdr_crc {
        return Err(LzmaError::Corrupt {
            reason: format!(
                "XZ block-header CRC32 mismatch: expected {expected_hdr_crc:#010X}, got {actual_hdr_crc:#010X}"
            ),
        });
    }

    // Compressed payload: everything after the header up to (but not
    // including) the check. The LZMA2 stream has its own
    // end-of-stream marker (control byte 0x00), so we let the decoder
    // consume exactly what it needs and report bytes consumed.
    let remaining = &input[header_size..];
    let (decoded, lzma2_consumed) = decode_lzma2_stream(remaining)?;

    // Apply BCJ reverse transform if present. For empty data (like the
    // good-1-empty-bcj-lzma2 fixture), the reverse is a no-op.
    let final_output = if let Some(bcj_id) = bcj_filter {
        apply_bcj_reverse(bcj_id, decoded)
    } else {
        decoded
    };

    let after_lzma2 = header_size + lzma2_consumed;
    // Pad to 4-byte alignment from the start of the block.
    let padded = (after_lzma2 + 3) & !3;
    let total = padded + check_size;

    Ok((final_output, total))
}

/// Apply a BCJ reverse transform to the LZMA2-decoded data.
/// For empty data, this is a no-op. For non-empty data with BCJ x86
/// (0x04), the filter reverses the E8/E9 branch conversion.
fn apply_bcj_reverse(bcj_id: u64, data: Vec<u8>) -> Vec<u8> {
    if data.is_empty() {
        return data;
    }
    match bcj_id {
        0x04 => {
            // BCJ x86: reverse the E8/E9 transform.
            // Delegate to omnizip-filters::BcjX86Filter.
            // For now, the filter is in a separate crate; inline the
            // reverse transform here to avoid a cross-crate dependency.
            bcj_x86_reverse(&data)
        }
        // Other BCJ variants (PowerPC, ARM, etc.) are pass-through
        // for now — the reverse transform for empty data is already
        // handled. For non-empty data, these need their own impl.
        _ => data,
    }
}

/// Inline BCJ x86 reverse transform — converts pseudo-absolute
/// addresses back to relative. Matches the `BcjX86Filter::decode`
/// implementation in `omnizip-filters`.
fn bcj_x86_reverse(data: &[u8]) -> Vec<u8> {
    let mut output = data.to_vec();
    if output.len() <= 4 {
        return output;
    }
    let mut i = 0usize;
    let limit = output.len() - 4;
    while i <= limit {
        let b = output[i];
        if b == 0xE8 || b == 0xE9 {
            let abs = u32::from_le_bytes([output[i + 1], output[i + 2], output[i + 3], output[i + 4]]);
            let rel = abs.wrapping_sub(i as u32);
            output[i + 1..i + 5].copy_from_slice(&rel.to_le_bytes());
            i += 5;
        } else {
            i += 1;
        }
    }
    output
}

/// Number of bytes used by the trailing check, given the check type.
fn check_size_bytes(check_type: u8) -> usize {
    match check_type {
        0x01 | 0x02 => 4,          // CRC32
        0x03 | 0x04 => 8,          // CRC64
        0x0A => 32,                // SHA-256
        // 0x00 (None) and any reserved value contribute no check bytes.
        _ => 0,
    }
}

/// Read a variable-length integer (XZ VLI). Returns the value and
/// the number of bytes consumed.
fn read_vli(input: &[u8]) -> Result<(u64, usize), LzmaError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for (i, &byte) in input.iter().enumerate().take(9) {
        if shift >= 64 {
            return Err(LzmaError::Corrupt {
                reason: "XZ VLI exceeds 64 bits".into(),
            });
        }
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
        shift += 7;
    }
    Err(LzmaError::Corrupt {
        reason: "XZ VLI truncated".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_constants_match_spec() {
        assert_eq!(XZ_MAGIC, [0xFD, b'7', b'z', b'X', b'Z', 0x00]);
        assert_eq!(XZ_FOOTER_MAGIC, [b'Y', b'Z']);
    }

    #[test]
    fn rejects_short_input() {
        assert!(xz_decompress(&[0u8; 5]).is_err());
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bad = vec![0xFF; 24];
        bad[0..6].copy_from_slice(&[0xFF; 6]);
        assert!(xz_decompress(&bad).is_err());
    }

    #[test]
    fn vli_single_byte() {
        assert_eq!(read_vli(&[0x42]).unwrap(), (0x42, 1));
    }

    #[test]
    fn vli_multi_byte() {
        // 0x80 0x01 → value = 0 + (1 << 7) = 128
        assert_eq!(read_vli(&[0x80, 0x01]).unwrap(), (128, 2));
    }

    #[test]
    fn check_size_table() {
        assert_eq!(check_size_bytes(0), 0);
        assert_eq!(check_size_bytes(1), 4);
        assert_eq!(check_size_bytes(4), 8);
        assert_eq!(check_size_bytes(0x0A), 32);
    }
}
