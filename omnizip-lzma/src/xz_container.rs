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
#[allow(dead_code)]
pub const STREAM_HEADER_SIZE: usize = 12;

/// Footer size in bytes.
#[allow(dead_code)]
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
    let mut output = Vec::new();
    let mut pos = 0usize;
    loop {
        let (out, consumed) = decode_stream(&input[pos..])?;
        output.extend_from_slice(&out);
        pos += consumed;
        if pos == input.len() {
            break;
        }
        // Stream Padding between (or after) streams: zero bytes, a
        // multiple of four. Anything else must be a new stream —
        // whose header parse then rejects garbage magic.
        let pad_start = pos;
        while pos < input.len() && input[pos] == 0 {
            pos += 1;
        }
        if (pos - pad_start) % 4 != 0 {
            return Err(LzmaError::Corrupt {
                reason: "XZ stream padding is not a multiple of 4".into(),
            });
        }
        if pos == input.len() {
            break;
        }
    }
    Ok(output)
}

/// Decode ONE stream (header + blocks + index + footer); returns the
/// concatenated block payload and the bytes consumed.
fn decode_stream(input: &[u8]) -> Result<(Vec<u8>, usize), LzmaError> {
    let (stream_flags, after_header) = parse_stream_header(input)?;
    let check_type = stream_flags & 0x0F;
    if !matches!(check_type, 0 | 1 | 4 | 0x0A) {
        return Err(LzmaError::Corrupt {
            reason: format!("XZ check type {check_type:#04X} is reserved"),
        });
    }
    let check_size = check_size_bytes(check_type);

    let mut output = Vec::new();
    let mut records: Vec<(u64, u64)> = Vec::new();
    let mut cursor = after_header;
    let index_start;
    loop {
        let room = input.len().saturating_sub(cursor);
        if room < STREAM_FOOTER_SIZE {
            return Err(LzmaError::Corrupt {
                reason: "XZ stream truncated before index".into(),
            });
        }
        if input[cursor] == 0x00 {
            index_start = cursor;
            break;
        }
        let (block_output, unpadded, uncompressed, after_block) =
            decode_block(&input[cursor..], check_type, check_size)?;

        output.extend_from_slice(&block_output);
        records.push((unpadded, uncompressed));
        cursor += after_block;
    }

    // Index: indicator + count + records + padding + CRC32.
    let index_end = validate_index(input, index_start, &records)?;

    // Footer: CRC32 over backward-size + flags; backward size must
    // equal the real index size; flags must match the header. The
    // footer follows the index DIRECTLY — in concatenated streams
    // there is more file after it, so end-of-input is wrong here.
    let footer_start = index_end;
    if footer_start + STREAM_FOOTER_SIZE > input.len() {
        return Err(LzmaError::Corrupt {
            reason: "XZ index overruns the stream".into(),
        });
    }
    let footer = &input[footer_start..];
    if footer[10..12] != XZ_FOOTER_MAGIC {
        return Err(LzmaError::Corrupt {
            reason: "XZ footer magic mismatch".into(),
        });
    }
    let expected_crc = u32::from_le_bytes([footer[0], footer[1], footer[2], footer[3]]);
    let actual_crc = crc32(&footer[4..10]);
    if expected_crc != actual_crc {
        return Err(LzmaError::Corrupt {
            reason: "XZ footer CRC32 mismatch".into(),
        });
    }
    let backward_size = u32::from_le_bytes([footer[4], footer[5], footer[6], footer[7]]);
    let index_size = footer_start - index_start;
    if (backward_size as usize + 1) * 4 != index_size {
        return Err(LzmaError::Corrupt {
            reason: format!(
                "XZ footer backward size says {} bytes of index, found {index_size}",
                (backward_size + 1) * 4
            ),
        });
    }
    if footer[8] != 0 || footer[9] != input[7] {
        return Err(LzmaError::Corrupt {
            reason: "XZ footer stream flags disagree with the header".into(),
        });
    }
    Ok((output, footer_start + STREAM_FOOTER_SIZE))
}

/// Validate the Index at `index_start`: indicator + record count +
/// (unpadded size, uncompressed size) per record + zero padding to
/// 4-byte alignment + CRC32. Returns the index end offset.
fn validate_index(
    input: &[u8],
    index_start: usize,
    records: &[(u64, u64)],
) -> Result<usize, LzmaError> {
    let mut cursor = index_start;
    if input.get(cursor) != Some(&0x00) {
        return Err(LzmaError::Corrupt {
            reason: "XZ index indicator missing".into(),
        });
    }
    cursor += 1;
    let (count, used) = read_vli(&input[cursor..])?;
    cursor += used;
    if count != records.len() as u64 {
        return Err(LzmaError::Corrupt {
            reason: format!(
                "XZ index declares {count} record(s) for {} block(s)",
                records.len()
            ),
        });
    }
    for &(block_unpadded, uncompressed) in records {
        let (u, used) = read_vli(&input[cursor..])?;
        cursor += used;
        let (c, used) = read_vli(&input[cursor..])?;
        cursor += used;
        // The xz-utils corpus pins the reference behavior: the
        // recorded unpadded size is validated against the block with
        // the check INCLUDED (header + compressed + check — the
        // corpus's good files carry exactly those values and the
        // "wrong Unpadded Sizes" bad file swaps two of them).
        let expected = block_unpadded;
        if u != expected {
            return Err(LzmaError::Corrupt {
                reason: format!("XZ index unpadded size {u} != {expected}"),
            });
        }
        if c != uncompressed {
            return Err(LzmaError::Corrupt {
                reason: "XZ index uncompressed size disagrees with the block".into(),
            });
        }
    }
    // Padding to 4-byte alignment, then the CRC32 over everything
    // from the indicator through the padding.
    let padded = (cursor + 3) & !3;
    let crc_at = padded;
    if crc_at + 4 > input.len() {
        return Err(LzmaError::Corrupt {
            reason: "XZ index CRC32 truncated".into(),
        });
    }
    if input[cursor..crc_at].iter().any(|&b| b != 0) {
        return Err(LzmaError::Corrupt {
            reason: "XZ index padding is non-zero".into(),
        });
    }
    let expected = u32::from_le_bytes([
        input[crc_at],
        input[crc_at + 1],
        input[crc_at + 2],
        input[crc_at + 3],
    ]);
    let actual = crc32(&input[index_start..crc_at]);
    if expected != actual {
        return Err(LzmaError::Corrupt {
            reason: "XZ index CRC32 mismatch".into(),
        });
    }
    Ok(crc_at + 4)
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
/// Returns the block's decompressed bytes, its unpadded size (header +
/// data + padding, no check — the Index records this), its
/// uncompressed size, and the bytes consumed.
#[allow(clippy::too_many_lines)]
fn decode_block(
    input: &[u8],
    check_type: u8,
    check_size: usize,
) -> Result<(Vec<u8>, u64, u64, usize), LzmaError> {
    if input.is_empty() {
        return Err(LzmaError::Corrupt {
            reason: "XZ block truncated".into(),
        });
    }
    if input[0] == 0 {
        return Err(LzmaError::Corrupt {
            reason: "XZ block header is 0 (index indicator)".into(),
        });
    }
    let header_size = usize::from(input[0] + 1) * 4;
    if header_size > input.len() {
        return Err(LzmaError::Corrupt {
            reason: format!("XZ block header size {header_size} exceeds input"),
        });
    }

    let block_flags = input[1];
    if block_flags & 0x3C != 0 {
        return Err(LzmaError::Corrupt {
            reason: format!("XZ block flags reserved bits set ({block_flags:#04X})"),
        });
    }
    let num_filters = usize::from((block_flags & 0x03) + 1);
    let has_compressed_size = (block_flags & 0x40) != 0;
    let has_uncompressed_size = (block_flags & 0x80) != 0;

    let mut cursor = 2usize;
    let mut declared_compressed: Option<u64> = None;
    let mut declared_uncompressed: Option<u64> = None;
    if has_compressed_size {
        let (vli, consumed) = read_vli(&input[cursor..])?;
        cursor += consumed;
        declared_compressed = Some(vli);
    }
    if has_uncompressed_size {
        let (vli, consumed) = read_vli(&input[cursor..])?;
        cursor += consumed;
        declared_uncompressed = Some(vli);
    }

    // Filters. The LAST filter must be LZMA2; delta/BCJ may precede
    // it. Anything else is unsupported.
    let mut bcj_filter: Option<u64> = None;
    let mut bcj_start_offset: u32 = 0;
    let mut delta_distances: Vec<usize> = Vec::new();
    let mut saw_lzma2 = false;
    let mut header_end = cursor;
    for filter_idx in 0..num_filters {
        let (filter_id, consumed) = read_vli(&input[cursor..])?;
        cursor += consumed;
        let (props_size, consumed2) = read_vli(&input[cursor..])?;
        cursor += consumed2;
        let props_size = props_size as usize;
        if cursor + props_size > header_size - 4 {
            return Err(LzmaError::Corrupt {
                reason: "XZ filter properties exceed block header".into(),
            });
        }
        let props = &input[cursor..cursor + props_size];
        cursor += props_size;
        header_end = cursor;

        match filter_id {
            0x21 => {
                // LZMA2: exactly one props byte, 0-40 (xz format
                // §5.3.2; 41+ is reserved).
                if props_size != 1 {
                    return Err(LzmaError::Corrupt {
                        reason: format!("XZ LZMA2 props must be 1 byte, got {props_size}"),
                    });
                }
                if props[0] > 40 {
                    return Err(LzmaError::Corrupt {
                        reason: format!(
                            "XZ LZMA2 dictionary size property {} is reserved",
                            props[0]
                        ),
                    });
                }
                saw_lzma2 = true;
            }
            0x03 => {
                // Delta: one props byte, distance = value + 1 (1-256).
                if props_size != 1 {
                    return Err(LzmaError::Corrupt {
                        reason: format!("XZ delta props must be 1 byte, got {props_size}"),
                    });
                }
                delta_distances.push(usize::from(props[0]) + 1);
            }
            0x04..=0x0A => {
                // BCJ filters: optional 4-byte little-endian start
                // offset. Only ARM64 carries it in the corpus; other
                // architectures with a non-zero offset stay
                // unsupported (loud, never silently wrong).
                let mut offset = 0u32;
                if props_size == 4 {
                    offset = u32::from_le_bytes([props[0], props[1], props[2], props[3]]);
                } else if props_size != 0 {
                    return Err(LzmaError::Corrupt {
                        reason: format!("XZ BCJ props size {props_size} invalid"),
                    });
                }
                if offset != 0 && filter_id != 0x0A {
                    return Err(LzmaError::Corrupt {
                        reason: "XZ BCJ start offset supported for ARM64 only".into(),
                    });
                }
                bcj_filter = Some(filter_id);
                bcj_start_offset = offset;
            }
            other => {
                return Err(LzmaError::Corrupt {
                    reason: format!("XZ filter 0x{other:X} not supported"),
                });
            }
        }
        let _ = filter_idx;
    }
    if !saw_lzma2 {
        return Err(LzmaError::Corrupt {
            reason: "XZ block chain does not end in the LZMA2 filter".into(),
        });
    }

    // Header padding (up to the CRC) must be zero.
    if input[header_end..header_size - 4].iter().any(|&b| b != 0) {
        return Err(LzmaError::Corrupt {
            reason: "XZ block-header padding is non-zero".into(),
        });
    }

    // Block-header CRC32.
    let header_bytes = &input[..header_size];
    let expected_hdr_crc = u32::from_le_bytes([
        header_bytes[header_size - 4],
        header_bytes[header_size - 3],
        header_bytes[header_size - 2],
        header_bytes[header_size - 1],
    ]);
    let actual_hdr_crc = crc32(&header_bytes[..header_size - 4]);
    if expected_hdr_crc != actual_hdr_crc {
        return Err(LzmaError::Corrupt {
            reason: format!(
                "XZ block-header CRC32 mismatch: expected {expected_hdr_crc:#010X}, got {actual_hdr_crc:#010X}"
            ),
        });
    }

    let remaining = &input[header_size..];
    let (decoded, lzma2_consumed) = decode_lzma2_stream(remaining)?;

    // Filters unpack in the REVERSE order of the chain: the encoder
    // applies delta/BCJ before LZMA2, so decode applies LZMA2, then
    // the pre-filters last-to-first. (good-1-3delta-lzma2 chains
    // three delta filters.)
    let mut final_output = decoded;
    if let Some(bcj_id) = bcj_filter {
        final_output = apply_bcj_reverse(bcj_id, final_output, bcj_start_offset);
    }
    for &dist in delta_distances.iter().rev() {
        final_output = apply_delta_reverse(&final_output, dist);
    }
    let final_len = final_output.len();

    // Declared sizes, when present, must match reality.
    if let Some(declared) = declared_compressed {
        if declared != lzma2_consumed as u64 {
            return Err(LzmaError::Corrupt {
                reason: format!("XZ block compressed size {declared} != actual {lzma2_consumed}"),
            });
        }
    }
    if let Some(declared) = declared_uncompressed {
        if declared != final_len as u64 {
            return Err(LzmaError::Corrupt {
                reason: format!("XZ block uncompressed size {declared} != actual {final_len}"),
            });
        }
    }

    let after_lzma2 = header_size + lzma2_consumed;
    // Padding to 4-byte alignment must be zeros. The padding (and
    // check) can run past a truncated input — bounds-check before
    // slicing (fuzz seed 1592648897 pinned the panic).
    let padded = (after_lzma2 + 3) & !3;
    let padding = input
        .get(after_lzma2..padded)
        .ok_or_else(|| LzmaError::Corrupt {
            reason: "XZ block padding truncated".into(),
        })?;
    if padding.iter().any(|&b| b != 0) {
        return Err(LzmaError::Corrupt {
            reason: "XZ block padding is non-zero".into(),
        });
    }
    let total = padded + check_size;

    if check_size > 0 {
        let stored = input
            .get(padded..padded + check_size)
            .ok_or_else(|| LzmaError::Corrupt {
                reason: "XZ block check truncated".into(),
            })?;
        let ok = match check_type {
            1 => stored == crc32(&final_output).to_le_bytes(),
            4 => {
                use omnizip_checksum::Checksum as _;
                let mut c = omnizip_checksum::crc64::Crc64Xz::new();
                c.update(&final_output);
                match c.finish() {
                    omnizip_checksum::Digest::U64(v) => stored == v.to_le_bytes(),
                    _ => unreachable!("crc64 digest"),
                }
            }
            0x0A => stored == omnizip_crypto::sha256(&final_output),
            other => {
                return Err(LzmaError::Corrupt {
                    reason: format!("unsupported XZ check type {other:#04X}"),
                });
            }
        };
        if !ok {
            return Err(LzmaError::Corrupt {
                reason: format!("XZ block check mismatch (type {check_type:#04X})"),
            });
        }
    }

    Ok((
        final_output,
        header_size as u64 + lzma2_consumed as u64 + check_size as u64,
        final_len as u64,
        total,
    ))
}

/// Apply a delta filter reverse transform. The XZ delta filter encodes
/// each byte as the difference from the byte `distance` positions back.
/// The reverse adds the previous byte back.
fn apply_delta_reverse(data: &[u8], distance_raw: usize) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let distance = distance_raw.max(1);
    let mut out = data.to_vec();
    for i in distance..out.len() {
        out[i] = out[i].wrapping_add(out[i - distance]);
    }
    out
}

/// Apply a BCJ reverse transform to the LZMA2-decoded data.
/// For empty data, this is a no-op. For non-empty data with BCJ x86
/// (0x04), the filter reverses the E8/E9 branch conversion.
fn apply_bcj_reverse(bcj_id: u64, data: Vec<u8>, start_offset: u32) -> Vec<u8> {
    use omnizip_filters::Filter as _;
    if data.is_empty() {
        return data;
    }
    if start_offset != 0 {
        // Only ARM64 reaches here with an offset (validated above).
        return omnizip_filters::bcj_arm64::BcjArm64StartOffset::new(start_offset).decode(&data);
    }
    match bcj_id {
        // xz filter IDs (spec §5.3): 0x04 x86, 0x05 PowerPC, 0x06 IA64,
        // 0x07 ARM, 0x08 ARM Thumb, 0x09 SPARC, 0x0A ARM64.
        0x04 => omnizip_filters::bcj_x86::BcjX86Filter.decode(&data),
        0x05 => omnizip_filters::bcj_powerpc::BcjPowerPcFilter.decode(&data),
        0x06 => omnizip_filters::bcj_ia64::BcjIa64Filter.decode(&data),
        0x07 => omnizip_filters::bcj_arm::BcjArmFilter.decode(&data),
        0x08 => omnizip_filters::bcj_arm_thumb::BcjArmThumbFilter.decode(&data),
        0x09 => omnizip_filters::bcj_sparc::BcjSparcFilter.decode(&data),
        0x0A => omnizip_filters::bcj_arm64::BcjArm64Filter.decode(&data),
        _ => data,
    }
}

/// Number of bytes used by the trailing check, given the check type.
fn check_size_bytes(check_type: u8) -> usize {
    match check_type {
        0x01 | 0x02 => 4, // CRC32
        0x03 | 0x04 => 8, // CRC64
        0x0A => 32,       // SHA-256
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
            // Minimal encoding: a terminator of 0 in a multi-byte VLI
            // means a shorter encoding carried the same value — the
            // reference rejects these (xz-utils bad-1-vli-1).
            if i > 0 && byte == 0 {
                return Err(LzmaError::Corrupt {
                    reason: "XZ VLI is not minimally encoded".into(),
                });
            }
            return Ok((value, i + 1));
        }
        if i == 8 {
            // Ninth byte still continuing: a VLI cannot exceed
            // 2^63-1 (xz format §1.2).
            return Err(LzmaError::Corrupt {
                reason: "XZ VLI exceeds 63 bits".into(),
            });
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
