//! LZ4 frame format encoder + decoder (pure-Rust, from spec).
//!
//! Implements the LZ4 frame format described at
//! <https://github.com/lz4/lz4/blob/dev/doc/lz4_Frame_format.md>.
//!
//! ## Frame layout
//!
//! ```text
//! Magic (4 bytes): 0x04 0x22 0x4D 0x18
//! FLG (1 byte)
//! BD  (1 byte)
//! [Content_Size (8 bytes)] — if FLG bit 3 set
//! [Dict_ID (4 bytes)]      — if FLG bit 0 set
//! [HC (1 byte)]            — if FLG bit 2 set
//! Data blocks:
//!   Block_Size (4 bytes LE, high bit = 1 for uncompressed)
//!   Block_Data (Block_Size bytes)
//! EndMark (4 bytes): 0x00 0x00 0x00 0x00
//! [Content_Checksum (4 bytes)] — if FLG bit 2 set
//! ```

#![forbid(unsafe_code)]

use super::block;

/// LZ4 frame magic number.
const MAGIC: [u8; 4] = [0x04, 0x22, 0x4D, 0x18];

/// FLG byte: version=01, BIndependence=1, BChecksum=0, CSize=0,
/// CChecksum=0, reserved=0.
const FLG_DEFAULT: u8 = 0b0110_0000;

/// BD byte: BlockMaxSize=7 (4MB), reserved=0.
const BD_DEFAULT: u8 = 0b0111_0000;

/// Compress `input` into an LZ4 frame.
#[must_use]
pub fn compress_frame(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + 16);
    out.extend_from_slice(&MAGIC);
    out.push(FLG_DEFAULT);
    out.push(BD_DEFAULT);

    // Single block containing the compressed data.
    let block = block::compress_block(input);
    let block_size = block.len() as u32;
    out.extend_from_slice(&block_size.to_le_bytes());
    out.extend_from_slice(&block);

    // EndMark.
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

/// Decompress an LZ4 frame.
///
/// # Errors
///
/// Returns `&'static str` on malformed input.
pub fn decompress_frame(compressed: &[u8]) -> Result<Vec<u8>, &'static str> {
    if compressed.len() < 7 {
        return Err("frame too short");
    }
    if compressed[..4] != MAGIC {
        return Err("bad magic");
    }

    let flg = compressed[4];
    let _bd = compressed[5];
    let mut i = 6usize;

    // Optional Content_Size.
    if flg & 0b0000_1000 != 0 {
        i += 8;
    }
    // Optional Dict_ID.
    if flg & 0b0000_0001 != 0 {
        i += 4;
    }
    // Optional HC.
    if flg & 0b0000_0100 != 0 {
        i += 1;
    }

    let mut out = Vec::new();
    loop {
        if i + 4 > compressed.len() {
            return Err("block header extends past input");
        }
        let block_size_raw = u32::from_le_bytes([
            compressed[i],
            compressed[i + 1],
            compressed[i + 2],
            compressed[i + 3],
        ]);
        i += 4;

        if block_size_raw == 0 {
            break; // EndMark.
        }

        let uncompressed_flag = block_size_raw & 0x8000_0000 != 0;
        let block_size = (block_size_raw & 0x7FFF_FFFF) as usize;
        if i + block_size > compressed.len() {
            return Err("block data extends past input");
        }

        if uncompressed_flag {
            out.extend_from_slice(&compressed[i..i + block_size]);
        } else {
            let decoded = block::decompress_block(&compressed[i..i + block_size], 0)?;
            out.extend(decoded);
        }
        i += block_size;
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_empty() {
        let compressed = compress_frame(b"");
        let decompressed = decompress_frame(&compressed).expect("decode");
        assert!(decompressed.is_empty());
    }

    #[test]
    fn frame_round_trip_text() {
        let input = b"the quick brown fox jumps over the lazy dog. ".repeat(10);
        let compressed = compress_frame(&input);
        assert!(compressed.len() < input.len() + 20);
        let decompressed = decompress_frame(&compressed).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn frame_round_trip_binary() {
        let input: Vec<u8> = (0..4096u32).map(|i| (i * 7 % 251) as u8).collect();
        let compressed = compress_frame(&input);
        let decompressed = decompress_frame(&compressed).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn frame_rejects_bad_magic() {
        let bad = [0xFFu8, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0];
        assert!(decompress_frame(&bad).is_err());
    }

    #[test]
    fn frame_round_trip_repetitive() {
        let input: Vec<u8> = vec![b'A'; 10_000];
        let compressed = compress_frame(&input);
        assert!(compressed.len() < input.len());
        let decompressed = decompress_frame(&compressed).expect("decode");
        assert_eq!(decompressed, input);
    }
}
