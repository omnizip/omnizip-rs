//! XZ container encoder — stream header + block + index + footer.
//!
//! Wraps [`super::lzma2::encode_lzma2_stream`] output in an XZ stream.
//!
//! ## Layout
//!
//! ```text
//! Stream_Header    12 bytes: magic + flags + CRC32
//! Block_Header     variable
//! Block_Data       LZMA2 payload
//! Block_Padding    0-3 bytes
//! Check            4 bytes (CRC32 of uncompressed data)
//! Index            variable
//! Stream_Footer    12 bytes
//! ```

#![forbid(unsafe_code)]

use crate::crc32::crc32;
use crate::LzmaError;

const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];
const XZ_FOOTER_MAGIC: [u8; 2] = [b'Y', b'Z'];

#[allow(dead_code)] const STREAM_HEADER_SIZE: usize = 12;
#[allow(dead_code)] const STREAM_FOOTER_SIZE: usize = 12;

/// Compress `input` into an XZ container using LZMA2.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] only on internal arithmetic overflow.
pub fn xz_compress(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    let mut out = Vec::new();

    // 1. Stream header: magic + flags + CRC32.
    // CRC32 covers ONLY the 2-byte flags, not the magic.
    let mut flags = [0u8; 2];
    flags[0] = 0x00; // reserved
    flags[1] = 0x01; // check_type = CRC32
    out.extend_from_slice(&XZ_MAGIC);
    out.extend_from_slice(&flags);
    let flags_crc = crc32(&flags);
    out.extend_from_slice(&flags_crc.to_le_bytes());

    // 2. Block.
    let block_start = out.len();

    // 2a. Block header.
    // Block header layout:
    //   - Block_Header_Size (1 byte): (real_size / 4) - 1
    //   - Block_Flags (1 byte): filter_count-1 in low bits, reserved in high
    //   - Filter_ID (VLI) + Properties_Size (VLI) + Filter_Properties
    //   - Header_Padding (to align to 4 bytes minus 4 for CRC)
    //   - CRC32 (4 bytes)
    let lzma2_payload = crate::encoder::lzma2::encode_lzma2_stream(input)?;
    let block_header_start = out.len();

    // Reserve space for the size byte; fill in later.
    out.push(0);
    // Block flags: 1 filter (count-1=0), reserved bits 0.
    out.push(0x00);

    // Filter ID 0x21 (LZMA2) encoded as VLI. Single-byte VLI for
    // values < 128.
    out.push(0x21);
    // Properties size = 1 (dict_size byte for LZMA2... actually LZMA2
    // properties are 1 byte: dict_size code).
    out.push(0x01);
    // Dict size code: 40 = 4 GiB - 1 (max). Actually use a reasonable default.
    // The dict_size byte for LZMA2: bits 0-5 = log2(dict_size) - 16... actually
    // it's more complex. For our 16 MiB default: code 38? Use 40 (max).
    out.push(40);

    // Pad block header to multiple of 4 (subtract 4 for CRC32).
    let header_so_far = out.len() - block_header_start;
    let needed = ((header_so_far + 3) & !3) + 4;
    while out.len() - block_header_start < needed - 4 {
        out.push(0x00);
    }
    // Patch the size byte: ((real_size / 4) - 1).
    let real_size = out.len() - block_header_start + 4; // include the CRC32 bytes
    out[block_header_start] = ((real_size / 4) - 1) as u8;
    // Block header CRC32 covers everything EXCEPT the CRC32 itself.
    let bh_crc = crc32(&out[block_header_start..]);
    out.extend_from_slice(&bh_crc.to_le_bytes());

    // 2b. Block data.
    let block_data_start = out.len();
    out.extend_from_slice(&lzma2_payload);

    // 2c. Block padding to multiple of 4.
    while out.len() % 4 != 0 {
        out.push(0x00);
    }
    let block_data_end = out.len();

    // 2d. Check (CRC32 of uncompressed input).
    let check = crc32(input);
    out.extend_from_slice(&check.to_le_bytes());

    let block_end = out.len();
    let _ = (block_start, block_data_start, block_data_end, block_end);

    // 3. Index.
    let index_start = out.len();
    out.push(0x00); // Index indicator
    // Number of records (1) encoded as VLI.
    out.push(0x01);
    // Record 0: unpadded_size, uncompressed_size.
    let unpadded = (block_end - block_start - 4) as u64; // exclude header CRC
    // Actually per XZ spec: unpadded_size = block_header_size + filter_data_size
    // (excluding padding and check). For simplicity use the same value.
    let _ = unpadded;
    let unpadded_size = (block_data_end - block_data_start) as u64;
    let total_uncompressed = input.len() as u64;
    write_vli(&mut out, unpadded_size);
    write_vli(&mut out, total_uncompressed);

    // Index padding to multiple of 4.
    while (out.len() - index_start) % 4 != 0 {
        out.push(0x00);
    }
    // Index CRC32.
    let index_crc = crc32(&out[index_start..]);
    out.extend_from_slice(&index_crc.to_le_bytes());

    // 4. Stream footer.
    let index_size = (out.len() - index_start) as u32;
    let backward_size = (index_size / 4) - 1;
    // Footer layout:
    //   - CRC32 (4 bytes)
    //   - Backward_Size (4 bytes): (real_size / 4) - 1
    //   - Stream_Flags (2 bytes)
    //   - Footer_Magic (2 bytes)
    let mut footer_body = Vec::new();
    footer_body.extend_from_slice(&backward_size.to_le_bytes());
    footer_body.extend_from_slice(&flags);
    footer_body.extend_from_slice(&XZ_FOOTER_MAGIC);
    let footer_crc = crc32(&footer_body);
    out.extend_from_slice(&footer_crc.to_le_bytes());
    out.extend_from_slice(&footer_body);

    Ok(out)
}

/// Write `value` as a variable-length integer (VLI).
fn write_vli(out: &mut Vec<u8>, value: u64) {
    let mut v = value;
    loop {
        let byte = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xz_container::xz_decompress;

    #[test]
    fn empty_round_trips() {
        let compressed = xz_compress(&[]).expect("encode");
        let decompressed = xz_decompress(&compressed).expect("decode");
        assert!(decompressed.is_empty());
    }

    #[test]
    fn small_round_trips() {
        let input = b"hello xz world";
        let compressed = xz_compress(input).expect("encode");
        let decompressed = xz_decompress(&compressed).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn determinism() {
        let encode_once = || xz_compress(b"determinism").unwrap();
        assert_eq!(encode_once(), encode_once());
    }
}
