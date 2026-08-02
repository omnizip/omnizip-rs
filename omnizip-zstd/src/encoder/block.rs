//! ZSTD compressed-block encoder. Orchestrates the match finder,
//! literals encoder, and sequences encoder into a Compressed_Block
//! (block_type = 2).
//!
//! For each 128 KiB chunk of input:
//! 1. Run `match_finder::compress_block_fast` → SeqStore.
//! 2. Encode literals section (Raw, RLE, or Huffman).
//! 3. Encode sequences section (Predefined FSE tables).
//! 4. Choose Raw / RLE / Compressed, whichever produces the smallest
//!    block content.

#![forbid(unsafe_code)]

use crate::constants::{BLOCK_TYPE_COMPRESSED, BLOCK_TYPE_RAW, BLOCK_TYPE_RLE};
use crate::encoder::match_finder::{compress_block_with_min_match, MatchState, SeqStore};
use crate::encoder::sequences::encode_section;
use crate::xxhash;
use crate::ZstdError;

/// Maximum block content size (128 KiB per ZSTD spec). Use 127 KiB to
/// avoid edge cases where some decoders reject exactly-128KiB blocks.
pub(crate) const BLOCK_MAX_SIZE: usize = 127 * 1024;

/// Encode `plaintext` as a complete ZSTD frame with compressed blocks.
///
/// Compressed block encoder: match finder + FSE sequences + Huffman/Raw literals.
/// blocks. The output is a valid ZSTD frame that round-trips through
/// any decoder.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on internal failures.
pub fn encode_frame_compressed(
    plaintext: &[u8],
    level: u8,
) -> Result<Vec<u8>, ZstdError> {
    let params = crate::encoder::cparams::get_params(level);
    let mut out = Vec::with_capacity(plaintext.len() / 2 + 64);
    let mut match_state = MatchState::new(params.hash_log);

    // Magic.
    out.extend_from_slice(&crate::constants::MAGIC_BYTES);

    // Frame header: descriptor + window_descriptor + 8-byte FCS.
    write_frame_header(&mut out, plaintext.len());

    // Blocks.
    let mut rep_offsets = [1u32, 4, 8];
    let mut offset = 0;
    while offset < plaintext.len() {
        let remaining = plaintext.len() - offset;
        let chunk_size = remaining.min(BLOCK_MAX_SIZE);
        let is_last = offset + chunk_size == plaintext.len();
        let chunk = &plaintext[offset..offset + chunk_size];

        write_block(&mut out, chunk, is_last, &mut match_state, &mut rep_offsets, &params)?;
        offset += chunk_size;
    }

    // If input is empty, emit a single empty last Raw block.
    if plaintext.is_empty() {
        let hdr: u32 = 1; // last=1, type=Raw, size=0
        out.extend_from_slice(&hdr.to_le_bytes()[..3]);
    }

    // Content checksum (XXHash64 truncated to u32).
    let checksum = xxhash::zstd_frame_checksum(plaintext);
    out.extend_from_slice(&checksum.to_le_bytes());

    Ok(out)
}

/// Write the frame header using the smallest FCS encoding that fits,
/// matching the C reference's priority order.
///
/// Descriptor byte layout:
/// - bits 0-1: Dictionary_ID_flag (always 0 — no Dict_ID).
/// - bit 2: Content_Checksum_flag (always 1 — we always emit the checksum).
/// - bit 5: Single_Segment_flag.
/// - bits 6-7: Frame_Content_Size_flag (0/1/2/3 → 0/2/4/8-byte FCS).
///
/// Encoding choice:
/// - size ≤ 255: FCS_flag=0 (1 byte), single_segment=1 → 2-byte header.
/// - size ≤ 65535: FCS_flag=1 (2 bytes), single_segment=1 → 3-byte header.
/// - size ≤ 2³²-1: FCS_flag=2 (4 bytes), single_segment=1 → 5-byte header.
/// - larger: FCS_flag=3 (8 bytes), window_descriptor → 10-byte header.
///
/// When single_segment=1, no window_descriptor is emitted (the window
/// size is implied to equal the frame content size).
fn write_frame_header(out: &mut Vec<u8>, uncompressed_size: usize) {
    let size_u64 = uncompressed_size as u64;

    // Pick the smallest encoding.
    let (fcs_flag, fcs_bytes_buf, single_segment): (u8, [u8; 8], bool) = if size_u64 <= 255 {
        (0, size_u64.to_le_bytes(), true)
    } else if size_u64 <= 65_791 {
        // FCS_Type=1: 2-byte field. Decoder adds 256, so subtract here.
        let stored = size_u64 - 256;
        (1, stored.to_le_bytes(), true)
    } else if size_u64 <= u32::MAX as u64 {
        (2, size_u64.to_le_bytes(), true)
    } else {
        // Need window_descriptor + 8-byte FCS for > 4 GiB inputs.
        let window_log: u32 = 64 - size_u64.saturating_sub(1).leading_zeros();
        let window_log = window_log.max(10).min(31);
        let window_descriptor: u8 = ((window_log - 10) as u8) << 3;
        let descriptor: u8 = (3u8 << 6) | 0x04;
        out.push(descriptor);
        out.push(window_descriptor);
        out.extend_from_slice(&size_u64.to_le_bytes());
        return;
    };
    let fcs_len = match fcs_flag {
        0 => 1,
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let fcs_bytes = &fcs_bytes_buf[..fcs_len];

    let descriptor: u8 = (fcs_flag << 6)
        | 0x04 // content_checksum = 1
        | if single_segment { 0x20 } else { 0 };
    out.push(descriptor);
    out.extend_from_slice(fcs_bytes);
}

/// Write one block. Chooses Raw/RLE/Compressed based on which produces
/// the smallest output.
fn write_block(
    out: &mut Vec<u8>,
    chunk: &[u8],
    is_last: bool,
    ms: &mut MatchState,
    rep_offsets: &mut [u32; 3],
    params: &crate::encoder::cparams::CompressionParams,
) -> Result<(), ZstdError> {
    // Clear hash table: positions are block-relative, so cross-block
    // references would be invalid.
    ms.clear();

    // RLE check: entire chunk is one repeated byte.
    if chunk.len() >= 2 && chunk.iter().all(|&b| b == chunk[0]) {
        write_rle_block(out, chunk[0], chunk.len(), is_last);
        return Ok(());
    }

    // Try compressed block.
    let mut seq_store = SeqStore::new();
    seq_store.reset(*rep_offsets);
    // Use level-specific min_match from the compression parameters.
    // The C reference uses searchLength which ranges from 3 to 7.
    // Our hash is 4 bytes, so we clamp to >= 4.
    let min_match = params.min_match.max(4) as usize;
    compress_block_with_min_match(chunk, &mut seq_store, ms, min_match);
    *rep_offsets = seq_store.rep_offsets;

    let mut compressed_content = Vec::new();
    let encode_result = encode_compressed_content(&mut compressed_content, &seq_store);

    // Use compressed only if it's smaller than the raw chunk.
    // FSE bitstream is now correct (write_ncount flush fixed).
    let use_compressed = encode_result.is_ok()
        && compressed_content.len() < chunk.len();

    if use_compressed {
        write_compressed_block_header(out, compressed_content.len(), is_last);
        out.extend_from_slice(&compressed_content);
    } else {
        write_raw_block(out, chunk, is_last);
    }

    Ok(())
}

/// Encode the compressed block content: literals section + sequences
/// section. Tries both Raw and Huffman literals, picks the smaller.
fn encode_compressed_content(
    out: &mut Vec<u8>,
    seq_store: &SeqStore,
) -> Result<(), ZstdError> {
    // Build Raw literals section (always correct).
    let mut raw_literals = Vec::new();
    write_raw_literals(&mut raw_literals, &seq_store.literals);

    // Build Huffman literals section. The encoder falls back to Raw
    // internally when the alphabet is too large for direct weight
    // encoding (> 128 symbols) or the distribution is degenerate.
    let huf_literals = crate::huffman::encoder::encode_literals(&seq_store.literals)
        .unwrap_or_default();

    // Use Huffman when smaller than Raw.
    if !huf_literals.is_empty() && huf_literals.len() < raw_literals.len() {
        out.extend_from_slice(&huf_literals);
    } else {
        out.extend_from_slice(&raw_literals);
    }

    // Sequences section.
    if seq_store.sequences.is_empty() && seq_store.literals.is_empty() {
        out.push(0x00);
    } else {
        encode_section(out, seq_store)?;
    }

    Ok(())
}

/// Write a Raw literals section (block_type=0). Minimal header for
/// small literal counts.
fn write_raw_literals(out: &mut Vec<u8>, literals: &[u8]) {
    let lit_size = literals.len();
    // Use 1-byte header when lit_size fits in 5 bits (lit_size < 32).
    // block_type=0 (Raw), lhl_code determines header size.
    if lit_size < 32 {
        // 1-byte header: bits 0-1=block_type(0), bits 2-3=lhl_code(0),
        // bits 3-7=lit_size. So byte = lit_size << 3.
        out.push((lit_size << 3) as u8);
    } else if lit_size < 4096 {
        // 2-byte header: lhl_code=1, litSize = u16_LE >> 4.
        let lhc: u16 = ((lit_size as u16) << 4) | 0x04; // lhl_code=1 in bits 2-3
        out.extend_from_slice(&lhc.to_le_bytes());
    } else {
        // 3-byte header: lhl_code=3.
        let lhc: u32 = ((lit_size as u32) << 4) | 0x0C; // lhl_code=3 in bits 2-3
        out.extend_from_slice(&lhc.to_le_bytes()[..3]);
    }
    out.extend_from_slice(literals);
}

/// Write a Raw block header (3 bytes LE) + data.
fn write_raw_block(out: &mut Vec<u8>, data: &[u8], is_last: bool) {
    let hdr: u32 = usize::from(is_last) as u32
        | (u32::from(BLOCK_TYPE_RAW) << 1)
        | ((data.len() as u32) << 3);
    out.extend_from_slice(&hdr.to_le_bytes()[..3]);
    out.extend_from_slice(data);
}

/// Write an RLE block header + the repeated byte.
fn write_rle_block(out: &mut Vec<u8>, byte: u8, size: usize, is_last: bool) {
    let hdr: u32 = usize::from(is_last) as u32
        | (u32::from(BLOCK_TYPE_RLE) << 1)
        | ((size as u32) << 3);
    out.extend_from_slice(&hdr.to_le_bytes()[..3]);
    out.push(byte);
}

/// Write a Compressed block header (3 bytes LE).
fn write_compressed_block_header(out: &mut Vec<u8>, content_size: usize, is_last: bool) {
    let hdr: u32 = usize::from(is_last) as u32
        | (u32::from(BLOCK_TYPE_COMPRESSED) << 1)
        | ((content_size as u32) << 3);
    out.extend_from_slice(&hdr.to_le_bytes()[..3]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decompress;

    #[test]
    fn empty_input_round_trips() {
        let compressed = encode_frame_compressed(&[], 1).expect("encode");
        let decompressed = decompress(&compressed, 0).expect("decode");
        assert!(decompressed.is_empty());
    }

    #[test]
    fn short_input_round_trips() {
        let input = b"hello world";
        let compressed = encode_frame_compressed(input, 1).expect("encode");
        let decompressed = decompress(&compressed, input.len() as u32).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn repetitive_input_round_trips() {
        // 100 'A's — should use RLE block.
        let input: Vec<u8> = vec![b'A'; 100];
        let compressed = encode_frame_compressed(&input, 1).expect("encode");
        let decompressed = decompress(&compressed, input.len() as u32).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn pattern_input_round_trips() {
        // Repeated 8-byte pattern — should find matches.
        let input: Vec<u8> = (0..200).map(|i| b"abcdefgh"[(i % 8) as usize]).collect();
        let compressed = encode_frame_compressed(&input, 1).expect("encode");
        let decompressed = decompress(&compressed, input.len() as u32).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn larger_input_round_trips() {
        // 500 KiB of mixed data.
        let input: Vec<u8> = (0..500_000)
            .map(|i| {
                if i % 100 < 50 { (i % 26 + b'a' as i32) as u8 } else { (i % 256) as u8 }
            })
            .collect();
        let compressed = encode_frame_compressed(&input, 1).expect("encode");
        let decompressed = decompress(&compressed, input.len() as u32).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn determinism() {
        let input: Vec<u8> = (0..1000).map(|i| (i % 64) as u8).collect();
        let a = encode_frame_compressed(&input, 1).expect("encode");
        let b = encode_frame_compressed(&input, 1).expect("encode");
        assert_eq!(a, b, "encoder non-deterministic");
    }

    #[test]
    fn frame_header_uses_smallest_fcs() {
        // The magic + frame header should be compact for small inputs:
        //   4 bytes magic + 1 byte descriptor + 1 byte FCS = 6 bytes.
        let input = b"hello";
        let compressed = encode_frame_compressed(input, 1).expect("encode");
        // Magic (4) + frame header (2) + ... blocks ...
        // Verify descriptor byte: fcs_flag=0, single_segment=1, checksum=1.
        // descriptor = (0<<6) | 0x20 | 0x04 = 0x24.
        assert_eq!(compressed[4], 0x24, "expected 1-byte FCS + single_segment");
        assert_eq!(compressed[5], input.len() as u8, "FCS value = input length");
    }

    #[test]
    fn frame_header_2byte_fcs_for_medium_input() {
        // 1000 bytes: FCS_flag=1, 2-byte FCS.
        let input = vec![0u8; 1000];
        let compressed = encode_frame_compressed(&input, 1).expect("encode");
        // descriptor = (1<<6) | 0x20 | 0x04 = 0x64.
        assert_eq!(compressed[4], 0x64, "expected 2-byte FCS + single_segment");
    }

    #[test]
    fn full_byte_alphabet_round_trips() {
        // Binary data using all 256 byte values. The Huffman encoder
        // can't use direct weight encoding for > 128 symbols, so it
        // falls back to Raw literals inside compressed blocks.
        let input: Vec<u8> = (0..50_000).map(|i| (i % 256) as u8).collect();
        let compressed = encode_frame_compressed(&input, 1).expect("encode");
        let decompressed = decompress(&compressed, input.len() as u32).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn high_entropy_random_round_trips() {
        // Pseudo-random data — incompressible, should fall back to Raw
        // blocks throughout.
        let input: Vec<u8> = (0u32..10_000)
            .map(|i| {
                let x = i.wrapping_mul(2654435761) ^ (i >> 5);
                (x & 0xFF) as u8
            })
            .collect();
        let compressed = encode_frame_compressed(&input, 3).expect("encode");
        let decompressed = decompress(&compressed, input.len() as u32).expect("decode");
        assert_eq!(decompressed, input);
    }
}
