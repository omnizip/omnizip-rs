//! Brotli q=2..6 encoder — port of upstream `compress_fragment.c`.
//!
//! One-pass fast encoder that emits combined INSERT+COPY commands via
//! the 128-symbol command alphabet. Uses 8-byte hash for better match
//! quality than the 4-byte hash in `compress_fragment_two_pass`.
//!
//! Algorithm overview:
//! 1. Build literal prefix code from input histogram.
//! 2. Build command prefix code (128 symbols: insert/copy + distance).
//! 3. Scan input with hash table, finding 5+ byte matches.
//! 4. Emit commands directly to the bitstream (no intermediate storage).
//! 5. Optionally merge consecutive metablocks for better ratio.
//!
//! Vendored from upstream brotli (BSD-3-Clause), adapted to use our
//! `fast_encoder` helpers (BrotliWriteBits, HuffmanTree, etc.).

#![forbid(unsafe_code)]
#![allow(dead_code)] // Work in progress: q=2..6 encoder, not yet wired.
#![allow(
    non_snake_case,
    non_upper_case_globals,
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::cast_possible_truncation
)]

use crate::fast_encoder::{
    BrotliBuildAndStoreHuffmanTreeFast, BrotliStoreHuffmanTree, BrotliWriteBits, HuffmanTree,
    Log2FloorNonZero, BROTLI_UNALIGNED_LOAD32, BROTLI_UNALIGNED_LOAD64, FindMatchLengthWithLimit,
};

const MAX_DISTANCE: usize = (1usize << 18) - 16; // BROTLI_MAX_BACKWARD_LIMIT(18)
const K_HASH_MUL32: u64 = 0x1e35_a7bd;
const BROTLI_WINDOW_GAP: usize = 16;

fn Hash(p: &[u8], shift: usize) -> u32 {
    let h = (BROTLI_UNALIGNED_LOAD64(p) << 24).wrapping_mul(K_HASH_MUL32);
    (h >> shift) as u32
}

fn HashBytesAtOffset(v: u64, offset: usize, shift: usize) -> u32 {
    let h = ((v >> (8 * offset)) << 24).wrapping_mul(K_HASH_MUL32);
    (h >> shift) as u32
}

fn IsMatch(p1: &[u8], p2: &[u8]) -> bool {
    BROTLI_UNALIGNED_LOAD32(p1) == BROTLI_UNALIGNED_LOAD32(p2) && p1[4] == p2[4]
}

/// Build literal prefix code and store to bitstream. Returns the
/// estimated compression ratio (millibytes/char).
fn BuildAndStoreLiteralPrefixCode(
    input: &[u8],
    lit_depth: &mut [u8; 256],
    lit_bits: &mut [u16; 256],
    storage_ix: &mut usize,
    storage: &mut [u8],
    tree: &mut [HuffmanTree],
) -> usize {
    let mut histogram = [0u32; 256];
    let input_size = input.len();
    let mut histogram_total: u32;

    if input_size < (1 << 15) {
        for &b in input {
            histogram[b as usize] += 1;
        }
        histogram_total = input_size as u32;
        for i in 0..256 {
            let adjust = 2 * histogram[i].min(11);
            histogram[i] += adjust;
            histogram_total = histogram_total.wrapping_add(adjust);
        }
    } else {
        const K_SAMPLE_RATE: usize = 29;
        let mut i = 0;
        while i < input_size {
            histogram[input[i] as usize] += 1;
            i += K_SAMPLE_RATE;
        }
        histogram_total = ((input_size + K_SAMPLE_RATE - 1) / K_SAMPLE_RATE) as u32;
        for i in 0..256 {
            let adjust = 1 + 2 * histogram[i].min(11);
            histogram[i] += adjust;
            histogram_total = histogram_total.wrapping_add(adjust);
        }
    }

    BrotliBuildAndStoreHuffmanTreeFast(
        &histogram,
        histogram_total as usize,
        8u8,
        lit_depth,
        lit_bits,
        storage_ix,
        storage,
        tree,
    );

    let mut literal_ratio: u64 = 0;
    for i in 0..256 {
        if histogram[i] != 0 {
            literal_ratio += histogram[i] as u64 * lit_depth[i] as u64;
        }
    }
    // Estimated encoding ratio, millibytes per symbol.
    (literal_ratio * 125 / histogram_total as u64) as usize
}

/// Build command prefix code (128 symbols) and store to bitstream.
fn BuildAndStoreCommandPrefixCode(
    cmd_depth: &mut [u8; 128],
    cmd_bits: &mut [u16; 128],
    cmd_histo: &mut [u32; 128],
    tree: &mut [HuffmanTree],
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    let mut tmp_depth = [0u8; 704];
    let mut tmp_bits = [0u16; 704];

    BrotliCreateHuffmanTree(&cmd_histo[..64], 15, tree, &mut cmd_depth[..64]);
    BrotliCreateHuffmanTree(&cmd_histo[64..], 14, tree, &mut cmd_depth[64..]);

    // Scatter pattern: rearrange 128 command codes into the 704-symbol
    // command alphabet used by the decoder's kCmdLut.
    tmp_depth[..24].copy_from_slice(&cmd_depth[..24]);
    tmp_depth[24..32].copy_from_slice(&cmd_depth[40..48]);
    tmp_depth[32..40].copy_from_slice(&cmd_depth[24..32]);
    tmp_depth[40..48].copy_from_slice(&cmd_depth[48..56]);
    tmp_depth[48..56].copy_from_slice(&cmd_depth[32..40]);
    tmp_depth[56..64].copy_from_slice(&cmd_depth[56..64]);

    BrotliConvertBitDepthsToSymbols(&tmp_depth[..64], &mut tmp_bits[..64]);

    cmd_bits[..24].copy_from_slice(&tmp_bits[..24]);
    cmd_bits[24..32].copy_from_slice(&tmp_bits[32..40]);
    cmd_bits[32..40].copy_from_slice(&tmp_bits[48..56]);
    cmd_bits[40..48].copy_from_slice(&tmp_bits[24..32]);
    cmd_bits[48..56].copy_from_slice(&tmp_bits[40..48]);
    cmd_bits[56..64].copy_from_slice(&tmp_bits[56..64]);

    BrotliConvertBitDepthsToSymbols(&cmd_depth[64..], &mut tmp_bits[64..128]);
    cmd_bits[64..128].copy_from_slice(&tmp_bits[64..128]);

    // Build the full 704-symbol depth array for Huffman tree storage.
    let mut full_depth = [0u8; 704];
    full_depth[..8].copy_from_slice(&cmd_depth[..8]);
    full_depth[64..72].copy_from_slice(&cmd_depth[8..16]);
    full_depth[128..136].copy_from_slice(&cmd_depth[16..24]);
    full_depth[192..200].copy_from_slice(&cmd_depth[24..32]);
    full_depth[384..392].copy_from_slice(&cmd_depth[32..40]);
    for i in 0..8 {
        full_depth[128 + 8 * i] = cmd_depth[40 + i];
        full_depth[256 + 8 * i] = cmd_depth[48 + i];
        full_depth[448 + 8 * i] = cmd_depth[56 + i];
    }

    BrotliStoreHuffmanTree(&full_depth, 704, tree, storage_ix, storage);
    BrotliStoreHuffmanTree(&cmd_depth[64..], 64, tree, storage_ix, storage);
}

// --- Emit functions (write directly to bitstream) ---

fn EmitInsertLen(
    insertlen: usize,
    cmd_depth: &[u8; 128],
    cmd_bits: &[u16; 128],
    cmd_histo: &mut [u32; 128],
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    if insertlen < 6 {
        let code = insertlen + 40;
        BrotliWriteBits(cmd_depth[code] as usize, cmd_bits[code] as u64, storage_ix, storage);
        cmd_histo[code] += 1;
    } else if insertlen < 130 {
        let tail = insertlen - 2;
        let nbits = Log2FloorNonZero(tail as u64) - 1;
        let prefix = tail >> nbits;
        let inscode = (nbits as usize) * 2 + prefix + 42;
        BrotliWriteBits(cmd_depth[inscode] as usize, cmd_bits[inscode] as u64, storage_ix, storage);
        BrotliWriteBits(nbits as usize, (tail - (prefix << nbits)) as u64, storage_ix, storage);
        cmd_histo[inscode] += 1;
    } else if insertlen < 2114 {
        let tail = insertlen - 66;
        let nbits = Log2FloorNonZero(tail as u64);
        let code = nbits as usize + 50;
        BrotliWriteBits(cmd_depth[code] as usize, cmd_bits[code] as u64, storage_ix, storage);
        BrotliWriteBits(nbits as usize, (tail - (1usize << nbits)) as u64, storage_ix, storage);
        cmd_histo[code] += 1;
    } else {
        BrotliWriteBits(cmd_depth[61] as usize, cmd_bits[61] as u64, storage_ix, storage);
        BrotliWriteBits(12, (insertlen - 2114) as u64, storage_ix, storage);
        cmd_histo[61] += 1;
    }
}

fn EmitLongInsertLen(
    insertlen: usize,
    cmd_depth: &[u8; 128],
    cmd_bits: &[u16; 128],
    cmd_histo: &mut [u32; 128],
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    if insertlen < 22594 {
        BrotliWriteBits(cmd_depth[62] as usize, cmd_bits[62] as u64, storage_ix, storage);
        BrotliWriteBits(14, (insertlen - 6210) as u64, storage_ix, storage);
        cmd_histo[62] += 1;
    } else {
        BrotliWriteBits(cmd_depth[63] as usize, cmd_bits[63] as u64, storage_ix, storage);
        BrotliWriteBits(24, (insertlen - 22594) as u64, storage_ix, storage);
        cmd_histo[63] += 1;
    }
}

fn EmitCopyLen(
    copylen: usize,
    cmd_depth: &[u8; 128],
    cmd_bits: &[u16; 128],
    cmd_histo: &mut [u32; 128],
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    if copylen < 10 {
        BrotliWriteBits(cmd_depth[copylen + 14] as usize, cmd_bits[copylen + 14] as u64, storage_ix, storage);
        cmd_histo[copylen + 14] += 1;
    } else if copylen < 134 {
        let tail = copylen - 6;
        let nbits = Log2FloorNonZero(tail as u64) - 1;
        let prefix = tail >> nbits;
        let code = (nbits as usize) * 2 + prefix + 20;
        BrotliWriteBits(cmd_depth[code] as usize, cmd_bits[code] as u64, storage_ix, storage);
        BrotliWriteBits(nbits as usize, (tail - (prefix << nbits)) as u64, storage_ix, storage);
        cmd_histo[code] += 1;
    } else if copylen < 2118 {
        let tail = copylen - 70;
        let nbits = Log2FloorNonZero(tail as u64);
        let code = nbits as usize + 28;
        BrotliWriteBits(cmd_depth[code] as usize, cmd_bits[code] as u64, storage_ix, storage);
        BrotliWriteBits(nbits as usize, (tail - (1usize << nbits)) as u64, storage_ix, storage);
        cmd_histo[code] += 1;
    } else {
        BrotliWriteBits(cmd_depth[39] as usize, cmd_bits[39] as u64, storage_ix, storage);
        BrotliWriteBits(24, (copylen - 2118) as u64, storage_ix, storage);
        cmd_histo[39] += 1;
    }
}

fn EmitCopyLenLastDistance(
    copylen: usize,
    cmd_depth: &[u8; 128],
    cmd_bits: &[u16; 128],
    cmd_histo: &mut [u32; 128],
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    if copylen < 12 {
        BrotliWriteBits(cmd_depth[copylen - 4] as usize, cmd_bits[copylen - 4] as u64, storage_ix, storage);
        cmd_histo[copylen - 4] += 1;
    } else if copylen < 72 {
        let tail = copylen - 8;
        let nbits = Log2FloorNonZero(tail as u64) - 1;
        let prefix = tail >> nbits;
        let code = (nbits as usize) * 2 + prefix + 4;
        BrotliWriteBits(cmd_depth[code] as usize, cmd_bits[code] as u64, storage_ix, storage);
        BrotliWriteBits(nbits as usize, (tail - (prefix << nbits)) as u64, storage_ix, storage);
        cmd_histo[code] += 1;
    } else if copylen < 136 {
        let tail = copylen - 8;
        let code = (tail >> 5) + 30;
        BrotliWriteBits(cmd_depth[code] as usize, cmd_bits[code] as u64, storage_ix, storage);
        BrotliWriteBits(5, (tail & 31) as u64, storage_ix, storage);
        BrotliWriteBits(cmd_depth[64] as usize, cmd_bits[64] as u64, storage_ix, storage);
        cmd_histo[code] += 1;
        cmd_histo[64] += 1;
    } else if copylen < 2120 {
        let tail = copylen - 72;
        let nbits = Log2FloorNonZero(tail as u64);
        let code = nbits as usize + 28;
        BrotliWriteBits(cmd_depth[code] as usize, cmd_bits[code] as u64, storage_ix, storage);
        BrotliWriteBits(nbits as usize, (tail - (1usize << nbits)) as u64, storage_ix, storage);
        BrotliWriteBits(cmd_depth[64] as usize, cmd_bits[64] as u64, storage_ix, storage);
        cmd_histo[code] += 1;
        cmd_histo[64] += 1;
    } else {
        BrotliWriteBits(cmd_depth[39] as usize, cmd_bits[39] as u64, storage_ix, storage);
        BrotliWriteBits(24, (copylen - 2120) as u64, storage_ix, storage);
        BrotliWriteBits(cmd_depth[64] as usize, cmd_bits[64] as u64, storage_ix, storage);
        cmd_histo[39] += 1;
        cmd_histo[64] += 1;
    }
}

fn EmitDistance(
    distance: usize,
    cmd_depth: &[u8; 128],
    cmd_bits: &[u16; 128],
    cmd_histo: &mut [u32; 128],
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    let d = distance + 3;
    let nbits = Log2FloorNonZero(d as u64).saturating_sub(1);
    let prefix = (d >> nbits) & 1;
    let offset = (2 + prefix) << nbits;
    let distcode = 2 * nbits as usize + prefix as usize + 78;
    BrotliWriteBits(cmd_depth[distcode] as usize, cmd_bits[distcode] as u64, storage_ix, storage);
    BrotliWriteBits(nbits as usize, (d - offset) as u64, storage_ix, storage);
    cmd_histo[distcode] += 1;
}

fn EmitLiterals(
    input: &[u8],
    len: usize,
    lit_depth: &[u8; 256],
    lit_bits: &[u16; 256],
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    for j in 0..len {
        let lit = input[j] as usize;
        BrotliWriteBits(lit_depth[lit] as usize, lit_bits[lit] as u64, storage_ix, storage);
    }
}

// --- Metablock header + block management ---

fn BrotliStoreMetaBlockHeader(
    len: usize,
    is_uncompressed: bool,
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    let mut nibbles = 6;
    BrotliWriteBits(1, 0, storage_ix, storage); // ISLAST = 0
    if len <= (1 << 16) {
        nibbles = 4;
    } else if len <= (1 << 20) {
        nibbles = 5;
    }
    BrotliWriteBits(2, (nibbles - 4) as u64, storage_ix, storage);
    BrotliWriteBits(nibbles * 4, (len - 1) as u64, storage_ix, storage);
    BrotliWriteBits(1, if is_uncompressed { 1 } else { 0 }, storage_ix, storage);
}

fn UpdateBits(n_bits: usize, mut bits: u32, pos: usize, array: &mut [u8]) {
    let mut n_bits = n_bits;
    let mut pos = pos;
    while n_bits > 0 {
        let byte_pos = pos >> 3;
        let n_unchanged_bits = pos & 7;
        let n_changed_bits = n_bits.min(8 - n_unchanged_bits);
        let total_bits = n_unchanged_bits + n_changed_bits;
        let mask = (!(((1u32 << total_bits) - 1) as u32)) | ((1u32 << n_unchanged_bits) - 1);
        let unchanged_bits = array[byte_pos] as u32 & mask;
        let changed_bits = bits & ((1u32 << n_changed_bits) - 1);
        array[byte_pos] = ((changed_bits << n_unchanged_bits) | unchanged_bits) as u8;
        n_bits -= n_changed_bits;
        bits >>= n_changed_bits;
        pos += n_changed_bits;
    }
}

fn RewindBitPosition(new_storage_ix: usize, storage_ix: &mut usize, storage: &mut [u8]) {
    let bitpos = new_storage_ix & 7;
    let mask = (1u32 << bitpos) - 1;
    storage[new_storage_ix >> 3] &= mask as u8;
    *storage_ix = new_storage_ix;
}

const MIN_RATIO: usize = 980;

fn ShouldUseUncompressedMode(
    metablock_start_pos: usize,
    next_emit_pos: usize,
    insertlen: usize,
    literal_ratio: usize,
) -> bool {
    let compressed = next_emit_pos - metablock_start_pos;
    if compressed * 50 > insertlen {
        false
    } else {
        literal_ratio > MIN_RATIO
    }
}

fn EmitUncompressedMetaBlock(
    begin: usize,
    end: usize,
    input: &[u8],
    storage_ix_start: usize,
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    let len = end - begin;
    RewindBitPosition(storage_ix_start, storage_ix, storage);
    BrotliStoreMetaBlockHeader(len, true, storage_ix, storage);
    *storage_ix = (*storage_ix + 7) & !7;
    let byte_pos = *storage_ix >> 3;
    // Storage is pre-allocated to input.len() * 2 + 1024 by the caller,
    // which is always sufficient for uncompressed blocks.
    storage[byte_pos..byte_pos + len].copy_from_slice(&input[begin..end]);
    *storage_ix += len << 3;
    storage[*storage_ix >> 3] = 0;
}

const K_CMD_HISTO_SEED: [u32; 128] = [
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0,
];

/// Main entry point: compress `input` using the q=2..6 algorithm.
/// Returns a valid brotli stream that any conformant decoder accepts.
pub fn compress(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        // Empty input: emit ISLAST + ISLASTEMPTY.
        let mut storage = vec![0u8; 8];
        let mut storage_ix = 0usize;
        BrotliWriteBits(1, 1, &mut storage_ix, &mut storage); // islast
        BrotliWriteBits(1, 1, &mut storage_ix, &mut storage); // isempty
        storage_ix = (storage_ix + 7) & !7;
        storage.truncate(storage_ix >> 3);
        return storage;
    }

    let mut tree = vec![HuffmanTree::default(); 2 * 704 + 1];
    let mut storage = vec![0u8; input.len() * 2 + 1024];
    let mut storage_ix = 0usize;

    // Frame header: WBITS=22 (4MB window, brotli default).
    // Encoded as 4 bits: 0b1011 (bit0=1, NBL=5 → WBITS=17+5=22).
    BrotliWriteBits(4, 0b1011, &mut storage_ix, &mut storage);

    compress_fragment_fast(
        input,
        true, // is_last
        &mut tree,
        &mut storage_ix,
        &mut storage,
    );

    // Final ISLAST + ISLASTEMPTY.
    BrotliWriteBits(1, 1, &mut storage_ix, &mut storage);
    BrotliWriteBits(1, 1, &mut storage_ix, &mut storage);
    storage_ix = (storage_ix + 7) & !7;

    storage.truncate(storage_ix >> 3);
    storage
}

fn compress_fragment_fast(
    input: &[u8],
    is_last: bool,
    tree: &mut [HuffmanTree],
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    let input_size = input.len();
    let initial_storage_ix = *storage_ix;

    if input_size == 0 {
        return;
    }

    let table_bits: usize = 15;
    let shift = 64 - table_bits;
    let mut table = vec![0i32; 1 << table_bits];

    let mut lit_depth = [0u8; 256];
    let mut lit_bits = [0u16; 256];
    let mut cmd_depth = [0u8; 128];
    let mut cmd_bits = [0u16; 128];
    let mut cmd_histo = K_CMD_HISTO_SEED;

    let k_first_block_size: usize = 3 << 15;
    let k_merge_block_size: usize = 1 << 16;
    let k_input_margin_bytes: usize = BROTLI_WINDOW_GAP;
    let k_min_match_len: usize = 5;

    let mut input_pos: usize = 0;
    let mut remaining = input_size;
    let mut block_size = remaining.min(k_first_block_size);
    let mut total_block_size = block_size;
    let mut metablock_start: usize = 0;
    let mut mlen_storage_ix = *storage_ix + 3;

    // First metablock header.
    BrotliStoreMetaBlockHeader(block_size, false, storage_ix, storage);
    BrotliWriteBits(13, 0, storage_ix, storage); // No block splits, no contexts.

    let literal_ratio = BuildAndStoreLiteralPrefixCode(
        &input[..block_size],
        &mut lit_depth,
        &mut lit_bits,
        storage_ix,
        storage,
        tree,
    );

    BuildAndStoreCommandPrefixCode(
        &mut cmd_depth,
        &mut cmd_bits,
        &mut cmd_histo,
        tree,
        storage_ix,
        storage,
    );

    let mut next_emit: usize = 0;
    let base_ip: usize = 0;
    let mut last_distance: i32 = -1;

    loop {
        // emit_commands: reset histogram.
        cmd_histo = K_CMD_HISTO_SEED;

        let mut ip = input_pos;
        let ip_end = input_pos + block_size;

        if block_size >= k_input_margin_bytes {
            let len_limit = (block_size - k_min_match_len).min(remaining - k_input_margin_bytes);
            let ip_limit = input_pos + len_limit;
            let mut skip: u32 = 32;

            ip += 1; // ++ip from upstream's for(next_hash = Hash(++ip, shift))
            if ip >= ip_limit {
                // Fall through to emit_remainder.
            } else {
                let mut next_hash = Hash(&input[ip..], shift);

                'outer: loop {
                    let mut candidate: usize;

                    loop {
                        let hash = next_hash;
                        let bytes_between = (skip >> 5) as usize;
                        skip += 1;

                        let prev_ip = ip;
                        ip = prev_ip + bytes_between;

                        if ip > ip_limit {
                            break 'outer;
                        }

                        next_hash = Hash(&input[ip..], shift);

                        // Check last-distance candidate.
                        let ld_candidate = prev_ip.wrapping_sub(last_distance as usize);
                        if ld_candidate < input.len()
                            && prev_ip >= (last_distance as usize)
                            && IsMatch(&input[prev_ip..], &input[ld_candidate..])
                            && ld_candidate < prev_ip
                        {
                            table[hash as usize] = (prev_ip as i32) - (base_ip as i32);
                            candidate = ld_candidate;
                            ip = prev_ip;
                            break;
                        }

                        candidate = base_ip + table[hash as usize] as usize;
                        table[hash as usize] = (prev_ip as i32) - (base_ip as i32);
                        ip = prev_ip;

                        if IsMatch(&input[ip..], &input[candidate..]) {
                            break;
                        }
                    }

                    // Check distance.
                    if ip - candidate > MAX_DISTANCE {
                        continue;
                    }

                    // Emit match + literals.
                    let matched = 5 + FindMatchLengthWithLimit(
                        &input[candidate + 5..],
                        &input[ip + 5..],
                        ip_end - ip - 5,
                    );
                    let distance = ip - candidate;
                    let insert = ip - next_emit;
                    ip += matched;

                    if insert < 6210 {
                        EmitInsertLen(insert, &cmd_depth, &cmd_bits, &mut cmd_histo, storage_ix, storage);
                    } else if ShouldUseUncompressedMode(metablock_start, next_emit, insert, literal_ratio) {
                        EmitUncompressedMetaBlock(
                            metablock_start, ip, input,
                            mlen_storage_ix - 3, storage_ix, storage,
                        );
                        remaining -= ip - input_pos;
                        input_pos = ip;
                        next_emit = ip;
                        // Go to next_block.
                        break 'outer;
                    } else {
                        EmitLongInsertLen(insert, &cmd_depth, &cmd_bits, &mut cmd_histo, storage_ix, storage);
                    }

                    EmitLiterals(
                        &input[next_emit..],
                        insert,
                        &lit_depth,
                        &lit_bits,
                        storage_ix,
                        storage,
                    );

                    if distance as i32 == last_distance {
                        BrotliWriteBits(cmd_depth[64] as usize, cmd_bits[64] as u64, storage_ix, storage);
                        cmd_histo[64] += 1;
                    } else {
                        EmitDistance(distance, &cmd_depth, &cmd_bits, &mut cmd_histo, storage_ix, storage);
                        last_distance = distance as i32;
                    }

                    EmitCopyLenLastDistance(matched, &cmd_depth, &cmd_bits, &mut cmd_histo, storage_ix, storage);
                    next_emit = ip;

                    if ip >= ip_limit {
                        break 'outer;
                    }

                    // Update hash table with positions in the last copy.
                    if ip >= 3 {
                        let input_bytes = BROTLI_UNALIGNED_LOAD64(&input[ip - 3..]);
                        let prev_hash = HashBytesAtOffset(input_bytes, 0, shift);
                        let cur_hash = HashBytesAtOffset(input_bytes, 3, shift);
                        table[prev_hash as usize] = (ip as i32) - (base_ip as i32) - 3;
                        let prev_hash = HashBytesAtOffset(input_bytes, 1, shift);
                        table[prev_hash as usize] = (ip as i32) - (base_ip as i32) - 2;
                        let prev_hash = HashBytesAtOffset(input_bytes, 2, shift);
                        table[prev_hash as usize] = (ip as i32) - (base_ip as i32) - 1;

                        candidate = base_ip + table[cur_hash as usize] as usize;
                        table[cur_hash as usize] = (ip as i32) - (base_ip as i32);
                    } else {
                        break 'outer;
                    }

                    // Continue matching from current position.
                    while ip < ip_end && candidate < ip && IsMatch(&input[ip..], &input[candidate..]) {
                        let matched = 5 + FindMatchLengthWithLimit(
                            &input[candidate + 5..],
                            &input[ip + 5..],
                            ip_end - ip - 5,
                        );
                        if ip - candidate > MAX_DISTANCE {
                            break;
                        }
                        ip += matched;
                        last_distance = (ip - matched - candidate) as i32;

                        EmitCopyLen(matched, &cmd_depth, &cmd_bits, &mut cmd_histo, storage_ix, storage);
                        EmitDistance(
                            last_distance as usize,
                            &cmd_depth,
                            &cmd_bits,
                            &mut cmd_histo,
                            storage_ix,
                            storage,
                        );

                        next_emit = ip;
                        if ip >= ip_limit {
                            break 'outer;
                        }

                        if ip >= 3 {
                            let input_bytes = BROTLI_UNALIGNED_LOAD64(&input[ip - 3..]);
                            let prev_hash = HashBytesAtOffset(input_bytes, 0, shift);
                            let cur_hash = HashBytesAtOffset(input_bytes, 3, shift);
                            table[prev_hash as usize] = (ip as i32) - (base_ip as i32) - 3;
                            let prev_hash = HashBytesAtOffset(input_bytes, 1, shift);
                            table[prev_hash as usize] = (ip as i32) - (base_ip as i32) - 2;
                            let prev_hash = HashBytesAtOffset(input_bytes, 2, shift);
                            table[prev_hash as usize] = (ip as i32) - (base_ip as i32) - 1;
                            candidate = base_ip + table[cur_hash as usize] as usize;
                            table[cur_hash as usize] = (ip as i32) - (base_ip as i32);
                        } else {
                            break;
                        }
                    }

                    ip += 1;
                    if ip >= ip_limit {
                        break 'outer;
                    }
                    next_hash = Hash(&input[ip..], shift);
                }
            }
        }

        // emit_remainder.
        input_pos += block_size;
        remaining = remaining.saturating_sub(block_size);
        block_size = remaining.min(k_merge_block_size);

        // Emit remaining literals for this metablock.
        if next_emit < ip_end {
            let insert = ip_end - next_emit;
            if insert < 6210 {
                EmitInsertLen(insert, &cmd_depth, &cmd_bits, &mut cmd_histo, storage_ix, storage);
                EmitLiterals(&input[next_emit..], insert, &lit_depth, &lit_bits, storage_ix, storage);
            } else if ShouldUseUncompressedMode(metablock_start, next_emit, insert, literal_ratio) {
                EmitUncompressedMetaBlock(metablock_start, ip_end, input, mlen_storage_ix - 3, storage_ix, storage);
            } else {
                EmitLongInsertLen(insert, &cmd_depth, &cmd_bits, &mut cmd_histo, storage_ix, storage);
                EmitLiterals(&input[next_emit..], insert, &lit_depth, &lit_bits, storage_ix, storage);
            }
        }
        next_emit = ip_end;

        // next_block.
        if remaining > 0 {
            metablock_start = input_pos;
            block_size = remaining.min(k_first_block_size);
            total_block_size = block_size;
            mlen_storage_ix = *storage_ix + 3;
            BrotliStoreMetaBlockHeader(block_size, false, storage_ix, storage);
            BrotliWriteBits(13, 0, storage_ix, storage);

            let _ = BuildAndStoreLiteralPrefixCode(
                &input[input_pos..input_pos + block_size],
                &mut lit_depth,
                &mut lit_bits,
                storage_ix,
                storage,
                tree,
            );
            BuildAndStoreCommandPrefixCode(
                &mut cmd_depth,
                &mut cmd_bits,
                &mut cmd_histo,
                tree,
                storage_ix,
                storage,
            );
            next_emit = input_pos;
            last_distance = -1;
            continue;
        }
        break;
    }

    // If output is larger than uncompressed, rewrite.
    if *storage_ix - initial_storage_ix > 31 + (input_size << 3) {
        EmitUncompressedMetaBlock(0, input_size, input, initial_storage_ix, storage_ix, storage);
    }

    let _ = is_last;
}

// Helper wrappers for functions that take slightly different arg types
// in our fast_encoder vs upstream.

fn BrotliCreateHuffmanTree(
    data: &[u32],
    tree_limit: i32,
    tree: &mut [HuffmanTree],
    depth: &mut [u8],
) {
    // Delegate to fast_encoder's version (same algorithm).
    crate::fast_encoder::BrotliCreateHuffmanTree(data, data.len(), tree_limit, tree, depth);
}

fn BrotliConvertBitDepthsToSymbols(depth: &[u8], bits: &mut [u16]) {
    crate::fast_encoder::BrotliConvertBitDepthsToSymbols(depth, depth.len(), bits);
}
