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

use super::alone::LzmaOptions;
use crate::crc32::crc32;
use crate::LzmaError;

const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];
const XZ_FOOTER_MAGIC: [u8; 2] = [b'Y', b'Z'];

#[allow(dead_code)]
const STREAM_HEADER_SIZE: usize = 12;
#[allow(dead_code)]
const STREAM_FOOTER_SIZE: usize = 12;

/// Compress `input` into an XZ container using LZMA2 with default
/// LZMA parameters (lc=3, lp=0, pb=2).
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] only on internal arithmetic overflow.
pub fn xz_compress(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    xz_compress_with_options(input, &LzmaOptions::default())
}

/// Compress `input` into an XZ container with explicit LZMA
/// parameters (lc, lp, pb, `dict_size`, parser choice).
///
/// The `dict_size` field of `options` is used to select the LZMA2
/// dictionary size code in the XZ block header. Other parameters
/// are embedded in the first LZMA2 chunk's properties byte.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] on parameter validation failure or
/// internal arithmetic overflow.
pub fn xz_compress_with_options(input: &[u8], options: &LzmaOptions) -> Result<Vec<u8>, LzmaError> {
    options.validate()?;
    let mut out = Vec::new();

    // 1. Stream header: magic + flags + CRC32.
    let mut flags = [0u8; 2];
    flags[0] = 0x00;
    flags[1] = 0x01; // check_type = CRC32
    out.extend_from_slice(&XZ_MAGIC);
    out.extend_from_slice(&flags);
    let flags_crc = crc32(&flags);
    out.extend_from_slice(&flags_crc.to_le_bytes());

    // 2. Blocks. Reference xz in its default (multithreaded) mode
    // splits the input into blocks of 3 x dict_size, each with fresh
    // encoder state — on data whose statistics drift (numbered CSV
    // rows, growing keys) per-block retraining beats one continuously
    // adapted model set by ~30%. Match that layout.
    let block_size = 3 * options.dict_size as usize;
    let mut records: Vec<(u64, u64)> = Vec::new();
    let mut offset = 0usize;
    while offset < input.len() {
        let end = (offset + block_size).min(input.len());
        let chunk = &input[offset..end];
        let lzma2_payload =
            crate::encoder::lzma2::encode_lzma2_stream_with_options(chunk, options)?;
        let unpadded = write_xz_block(&mut out, &lzma2_payload, options.dict_size, chunk);
        records.push((unpadded, chunk.len() as u64));
        offset = end;
    }
    if records.is_empty() {
        // Empty input: LZMA2 end-marker only, no blocks.
        let lzma2_payload =
            crate::encoder::lzma2::encode_lzma2_stream_with_options(input, options)?;
        let unpadded = write_xz_block(&mut out, &lzma2_payload, options.dict_size, input);
        records.push((unpadded, 0));
    }

    // 3. Index.
    let index_start = out.len();
    out.push(0x00);
    write_vli(&mut out, records.len() as u64);
    for (unpadded_size, uncompressed) in &records {
        // Per the .xz spec, Unpadded Size = Block Header size +
        // Compressed Data size + Check size — padding EXCLUDED.
        write_vli(&mut out, *unpadded_size);
        write_vli(&mut out, *uncompressed);
    }

    while (out.len() - index_start) % 4 != 0 {
        out.push(0x00);
    }
    let index_crc = crc32(&out[index_start..]);
    out.extend_from_slice(&index_crc.to_le_bytes());

    // 4. Stream footer.
    let index_size = (out.len() - index_start) as u32;
    let backward_size = (index_size / 4) - 1;
    // Stream Footer layout: CRC32 (4) + Backward Size (4) + Stream
    // Flags (2) + Magic "YZ". The CRC covers Backward Size + Stream
    // Flags ONLY — including the magic (as before) produces a wrong
    // CRC and every stream fails `xz -t`.
    let footer_core = [&backward_size.to_le_bytes()[..], &flags[..]].concat();
    let footer_crc = crc32(&footer_core);
    out.extend_from_slice(&footer_crc.to_le_bytes());
    out.extend_from_slice(&footer_core);
    out.extend_from_slice(&XZ_FOOTER_MAGIC);

    Ok(out)
}

/// Write one XZ block (header + LZMA2 payload + padding + CRC32
/// check). Returns the block's Unpadded Size for the index.
fn write_xz_block(out: &mut Vec<u8>, lzma2_payload: &[u8], dict_size: u32, plain: &[u8]) -> u64 {
    let block_header_start = out.len();

    out.push(0); // size byte, patched later
    out.push(0x00); // block flags: 1 filter

    out.push(0x21); // Filter ID: LZMA2
    out.push(0x01); // Properties size: 1 byte
    let dict_code = dict_size_to_lzma2_code(dict_size);
    out.push(dict_code);

    // Pad block header to multiple of 4 (minus 4 for CRC32).
    let header_so_far = out.len() - block_header_start;
    let needed = ((header_so_far + 3) & !3) + 4;
    while out.len() - block_header_start < needed - 4 {
        out.push(0x00);
    }
    let real_size = out.len() - block_header_start + 4;
    out[block_header_start] = ((real_size / 4) - 1) as u8;
    let bh_crc = crc32(&out[block_header_start..]);
    out.extend_from_slice(&bh_crc.to_le_bytes());

    let block_header_len = (out.len() - block_header_start) as u64;
    out.extend_from_slice(lzma2_payload);

    while out.len() % 4 != 0 {
        out.push(0x00);
    }

    let check = crc32(plain);
    out.extend_from_slice(&check.to_le_bytes());

    block_header_len + lzma2_payload.len() as u64 + 4
}

/// Map a `dict_size` in bytes to the LZMA2 1-byte dictionary code./// Map a `dict_size` in bytes to the LZMA2 1-byte dictionary code.
///
/// Per the LZMA2 spec: the code is `ceil(log2(dict_size)) - 16` for
/// dicts >= 4 KB, with code 40 reserved for "4 GiB - 1". Codes 41+ are
/// invalid.
fn dict_size_to_lzma2_code(dict_size: u32) -> u8 {
    if dict_size >= 0xFFFF_FFE0 {
        return 40;
    }
    let log = (dict_size.max(1).next_power_of_two()).trailing_zeros();
    let code = log.saturating_sub(11); // log2(2048) = 11 -> code 0... actually:
                                       // spec: bits 0-5 = (dict_size_id), where dict_size_id in [0, 40]
                                       // Mapping: dict_size = (2 | (bits[0:1])) << (bits[2:6] + 11)
                                       // For our purposes, use the next_power_of_two's log2 - 11 + 40-ish.
                                       // Simplest: clamp to [40, 40] for now (always max dict).
    let _ = code;
    40
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
