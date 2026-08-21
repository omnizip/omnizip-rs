//! Vendored Brotli encoder — adapted from upstream brotli crate (BSD-3-Clause).
//! All upstream code rewritten to use Vec instead of Allocator<HuffmanTree>.
#![allow(
    dead_code,
    non_snake_case,
    unused_parens,
    unused_assignments,
    unused_variables,
    non_upper_case_globals,
    non_camel_case_types,
    clippy::too_many_arguments,
    clippy::needless_range_loop
)]
use std::cmp::min;

// ── Constants ──

const MAX_HUFFMAN_BITS: usize = 15;
const kCompressFragmentTwoPassBlockSize: usize = 1 << 17;
const K_HASH_MUL32: u64 = 0x1e35_a7bd;

// ── HuffmanTree ──

#[derive(Clone, Copy, Default)]
pub struct HuffmanTree {
    pub total_count_: u32,
    pub index_left_: i16,
    pub index_right_or_value_: i16,
}
impl HuffmanTree {
    #[must_use]
    pub fn new(count: u32, left: i16, right: i16) -> Self {
        Self {
            total_count_: count,
            index_left_: left,
            index_right_or_value_: right,
        }
    }
}

// ── Utility functions ──

pub(crate) fn Log2FloorNonZero(n: u64) -> u32 {
    n.ilog2()
}

fn memcpy<T: Clone>(dst: &mut [T], dst_offset: usize, src: &[T], src_offset: usize, size: usize) {
    dst[dst_offset..dst_offset + size].clone_from_slice(&src[src_offset..src_offset + size]);
}

pub fn BrotliWriteBits(n_bits: usize, bits: u64, pos: &mut usize, array: &mut [u8]) {
    let p = &mut array[(*pos >> 3)..];
    let mut v: u64 = u64::from(p[0]);
    v |= bits << (*pos & 7);
    for i in 0..p.len().min(8) {
        p[i] = (v >> (8 * i)) as u8;
    }
    *pos = pos.wrapping_add(n_bits);
}

pub(crate) fn BROTLI_UNALIGNED_LOAD32(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}
pub(crate) fn BROTLI_UNALIGNED_LOAD64(p: &[u8]) -> u64 {
    let mut v = 0u64;
    for i in 0..8.min(p.len()) {
        v |= u64::from(p[i]) << (8 * i);
    }
    v
}
pub(crate) fn FindMatchLengthWithLimit(s1: &[u8], s2: &[u8], limit: usize) -> usize {
    let mut i = 0;
    while i < limit && i < s1.len() && i < s2.len() && s1[i] == s2[i] {
        i += 1;
    }
    i
}

// ── Bit reversal for Huffman codes ──

fn BrotliReverseBits(num_bits: usize, mut bits: u16) -> u16 {
    static LUT: [u16; 16] = [
        0x0, 0x8, 0x4, 0xc, 0x2, 0xa, 0x6, 0xe, 0x1, 0x9, 0x5, 0xd, 0x3, 0xb, 0x7, 0xf,
    ];
    let mut retval: u16 = LUT[(bits & 0xf) as usize];
    let mut i = 4;
    while i < num_bits {
        retval <<= 4;
        bits >>= 4;
        retval |= LUT[(bits & 0xf) as usize];
        i += 4;
    }
    retval
        >> if num_bits & 3 != 0 {
            4 - (num_bits & 3)
        } else {
            0
        }
}

// ── Huffman tree depth setting ──

fn BrotliSetDepth(p0: i32, pool: &mut [HuffmanTree], depth: &mut [u8], max_depth: i32) -> bool {
    let mut stack: [i32; 16] = [0; 16];
    let mut level: i32 = 0;
    let mut p: i32 = p0;
    stack[0] = -1;
    loop {
        if i32::from(pool[p as usize].index_left_) >= 0 {
            level += 1;
            if level > max_depth {
                return false;
            }
            stack[level as usize] = i32::from(pool[p as usize].index_right_or_value_);
            p = i32::from(pool[p as usize].index_left_);
            continue;
        } else {
            let pp = pool[p as usize];
            depth[pp.index_right_or_value_ as usize] = level as u8;
        }
        while level >= 0 && stack[level as usize] == -1 {
            level -= 1;
        }
        if level < 0 {
            return true;
        }
        p = stack[level as usize];
        stack[level as usize] = -1;
    }
}

// ── Huffman comparator + sort ──

trait HuffmanComparator {
    fn Cmp(&self, v0: &HuffmanTree, v1: &HuffmanTree) -> bool;
}
struct SimpleSort {}
impl HuffmanComparator for SimpleSort {
    fn Cmp(&self, v0: &HuffmanTree, v1: &HuffmanTree) -> bool {
        if v0.total_count_ == v1.total_count_ {
            v0.index_right_or_value_ > v1.index_right_or_value_
        } else {
            v0.total_count_ < v1.total_count_
        }
    }
}
fn SortHuffmanTreeItems(tree: &mut [HuffmanTree], n: usize, cmp: impl HuffmanComparator) {
    tree[..n].sort_by(|a, b| {
        if cmp.Cmp(a, b) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
}

// ── Build Huffman tree from histogram ──

pub(crate) fn BrotliCreateHuffmanTree(
    data: &[u32],
    length: usize,
    tree_limit: i32,
    tree: &mut [HuffmanTree],
    depth: &mut [u8],
) -> bool {
    let sentinel = HuffmanTree::new(u32::MAX, -1, -1);
    let mut count_limit: u32 = 1;
    loop {
        let mut node_index: usize = 0;
        let mut l = length;
        while l != 0 {
            l -= 1;
            if data[l] != 0 {
                let count = std::cmp::max(data[l], count_limit);
                tree[node_index] = HuffmanTree::new(count, -1, l as i16);
                node_index += 1;
            }
        }
        let n: usize = node_index;
        if n == 1 {
            depth[tree[0].index_right_or_value_ as usize] = 1u8;
            return true;
        }
        let mut i: usize = 0;
        let mut j: usize = n + 1;
        SortHuffmanTreeItems(tree, n, SimpleSort {});
        tree[n] = sentinel;
        tree[n + 1] = sentinel;
        let mut k = n - 1;
        while k != 0 {
            let left = if tree[i].total_count_ <= tree[j].total_count_ {
                let l = i;
                i += 1;
                l
            } else {
                let l = j;
                j += 1;
                l
            };
            let right = if tree[i].total_count_ <= tree[j].total_count_ {
                let r = i;
                i += 1;
                r
            } else {
                let r = j;
                j += 1;
                r
            };
            let j_end = 2 * n - k;
            tree[j_end] = HuffmanTree::new(
                tree[left]
                    .total_count_
                    .wrapping_add(tree[right].total_count_),
                left as i16,
                right as i16,
            );
            tree[j_end + 1] = sentinel;
            k -= 1;
        }
        if BrotliSetDepth((2 * n - 1) as i32, tree, depth, tree_limit) {
            return true;
        }
        count_limit = count_limit.wrapping_mul(2);
    }
}

// ── Convert depths to canonical Huffman codes (bit-reversed) ──

pub(crate) fn BrotliConvertBitDepthsToSymbols(depth: &[u8], len: usize, bits: &mut [u16]) {
    let mut bl_count = [0u16; MAX_HUFFMAN_BITS + 1];
    let mut next_code = [0u16; MAX_HUFFMAN_BITS + 1];
    for i in 0..len {
        bl_count[depth[i] as usize] += 1;
    }
    bl_count[0] = 0;
    let mut code: i32 = 0;
    for i in 1..=MAX_HUFFMAN_BITS {
        code = (code + i32::from(bl_count[i - 1])) << 1;
        next_code[i] = code as u16;
    }
    for i in 0..len {
        if depth[i] != 0 {
            bits[i] = BrotliReverseBits(depth[i] as usize, next_code[depth[i] as usize]);
            next_code[depth[i] as usize] += 1;
        }
    }
}

// ── RLE-encode code lengths for Huffman tree storage ──

fn Reverse(v: &mut [u8], mut start: usize, mut end: usize) {
    end -= 1;
    while start < end {
        v.swap(start, end);
        start += 1;
        end -= 1;
    }
}

fn decide_over_rle_use(depth: &[u8], length: usize) -> (bool, bool) {
    let mut total_reps_zero: usize = 0;
    let mut total_reps_non_zero: usize = 0;
    let mut count_reps_zero: usize = 1;
    let mut count_reps_non_zero: usize = 1;
    let mut i: usize = 0;
    while i < length {
        let value = depth[i];
        let mut reps: usize = 1;
        let mut k = i + 1;
        while k < length && depth[k] == value {
            reps += 1;
            k += 1;
        }
        if reps >= 3 && value == 0 {
            total_reps_zero += reps;
            count_reps_zero += 1;
        }
        if reps >= 4 && value != 0 {
            total_reps_non_zero += reps;
            count_reps_non_zero += 1;
        }
        i += reps;
    }
    let use_rle_for_non_zero = total_reps_non_zero > count_reps_non_zero.wrapping_mul(2);
    let use_rle_for_zero = total_reps_zero > count_reps_zero.wrapping_mul(2);
    (use_rle_for_non_zero, use_rle_for_zero)
}

fn BrotliWriteHuffmanTreeRepetitions(
    previous_value: u8,
    value: u8,
    mut repetitions: usize,
    tree_size: &mut usize,
    tree: &mut [u8],
    extra_bits_data: &mut [u8],
) {
    if previous_value != value {
        tree[*tree_size] = value;
        extra_bits_data[*tree_size] = 0;
        *tree_size += 1;
        repetitions -= 1;
    }
    if repetitions == 7 {
        tree[*tree_size] = value;
        extra_bits_data[*tree_size] = 0;
        *tree_size += 1;
        repetitions -= 1;
    }
    if repetitions < 3 {
        for _ in 0..repetitions {
            tree[*tree_size] = value;
            extra_bits_data[*tree_size] = 0;
            *tree_size += 1;
        }
    } else {
        let start = *tree_size;
        repetitions -= 3;
        loop {
            tree[*tree_size] = 16;
            extra_bits_data[*tree_size] = (repetitions & 0x03) as u8;
            *tree_size += 1;
            repetitions >>= 2;
            if repetitions == 0 {
                break;
            }
            repetitions -= 1;
        }
        Reverse(tree, start, *tree_size);
        Reverse(extra_bits_data, start, *tree_size);
    }
}

fn BrotliWriteHuffmanTreeRepetitionsZeros(
    mut repetitions: usize,
    tree_size: &mut usize,
    tree: &mut [u8],
    extra_bits_data: &mut [u8],
) {
    if repetitions == 11 {
        tree[*tree_size] = 0;
        extra_bits_data[*tree_size] = 0;
        *tree_size += 1;
        repetitions -= 1;
    }
    if repetitions < 3 {
        for _ in 0..repetitions {
            tree[*tree_size] = 0;
            extra_bits_data[*tree_size] = 0;
            *tree_size += 1;
        }
    } else {
        let start = *tree_size;
        repetitions -= 3;
        loop {
            tree[*tree_size] = 17;
            extra_bits_data[*tree_size] = (repetitions & 0x07) as u8;
            *tree_size += 1;
            repetitions >>= 3;
            if repetitions == 0 {
                break;
            }
            repetitions -= 1;
        }
        Reverse(tree, start, *tree_size);
        Reverse(extra_bits_data, start, *tree_size);
    }
}

fn BrotliWriteHuffmanTree(
    depth: &[u8],
    length: usize,
    tree: &mut [u8],
    extra_bits_data: &mut [u8],
    tree_size: &mut usize,
) {
    let mut previous_value: u8 = 8;
    let mut use_rle_for_non_zero = false;
    let mut use_rle_for_zero = false;
    let mut new_length: usize = length;
    let mut i: usize = 0;
    while i < length {
        if depth[length - i - 1] == 0 {
            new_length -= 1;
        } else {
            break;
        }
        i += 1;
    }
    if length > 50 {
        let (n, z) = decide_over_rle_use(depth, new_length);
        use_rle_for_non_zero = n;
        use_rle_for_zero = z;
    }
    i = 0;
    while i < new_length {
        let value = depth[i];
        let mut reps: usize = 1;
        if (value != 0 && use_rle_for_non_zero) || (value == 0 && use_rle_for_zero) {
            let mut k = i + 1;
            while k < new_length && depth[k] == value {
                reps += 1;
                k += 1;
            }
        }
        if value == 0 {
            BrotliWriteHuffmanTreeRepetitionsZeros(reps, tree_size, tree, extra_bits_data);
        } else {
            BrotliWriteHuffmanTreeRepetitions(
                previous_value,
                value,
                reps,
                tree_size,
                tree,
                extra_bits_data,
            );
            previous_value = value;
        }
        i += reps;
    }
}
static kCodeLengthBits: [u32; 18] = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 15, 31, 0, 11, 7];

static kCodeLengthDepth: [u8; 18] = [4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 0, 4, 4];

static kZeroRepsBits: [u64; 704] = [
    0x0, 0x0, 0x0, 0x7, 0x17, 0x27, 0x37, 0x47, 0x57, 0x67, 0x77, 0x770, 0xb87, 0x1387, 0x1b87,
    0x2387, 0x2b87, 0x3387, 0x3b87, 0x397, 0xb97, 0x1397, 0x1b97, 0x2397, 0x2b97, 0x3397, 0x3b97,
    0x3a7, 0xba7, 0x13a7, 0x1ba7, 0x23a7, 0x2ba7, 0x33a7, 0x3ba7, 0x3b7, 0xbb7, 0x13b7, 0x1bb7,
    0x23b7, 0x2bb7, 0x33b7, 0x3bb7, 0x3c7, 0xbc7, 0x13c7, 0x1bc7, 0x23c7, 0x2bc7, 0x33c7, 0x3bc7,
    0x3d7, 0xbd7, 0x13d7, 0x1bd7, 0x23d7, 0x2bd7, 0x33d7, 0x3bd7, 0x3e7, 0xbe7, 0x13e7, 0x1be7,
    0x23e7, 0x2be7, 0x33e7, 0x3be7, 0x3f7, 0xbf7, 0x13f7, 0x1bf7, 0x23f7, 0x2bf7, 0x33f7, 0x3bf7,
    0x1c387, 0x5c387, 0x9c387, 0xdc387, 0x11c387, 0x15c387, 0x19c387, 0x1dc387, 0x1cb87, 0x5cb87,
    0x9cb87, 0xdcb87, 0x11cb87, 0x15cb87, 0x19cb87, 0x1dcb87, 0x1d387, 0x5d387, 0x9d387, 0xdd387,
    0x11d387, 0x15d387, 0x19d387, 0x1dd387, 0x1db87, 0x5db87, 0x9db87, 0xddb87, 0x11db87, 0x15db87,
    0x19db87, 0x1ddb87, 0x1e387, 0x5e387, 0x9e387, 0xde387, 0x11e387, 0x15e387, 0x19e387, 0x1de387,
    0x1eb87, 0x5eb87, 0x9eb87, 0xdeb87, 0x11eb87, 0x15eb87, 0x19eb87, 0x1deb87, 0x1f387, 0x5f387,
    0x9f387, 0xdf387, 0x11f387, 0x15f387, 0x19f387, 0x1df387, 0x1fb87, 0x5fb87, 0x9fb87, 0xdfb87,
    0x11fb87, 0x15fb87, 0x19fb87, 0x1dfb87, 0x1c397, 0x5c397, 0x9c397, 0xdc397, 0x11c397, 0x15c397,
    0x19c397, 0x1dc397, 0x1cb97, 0x5cb97, 0x9cb97, 0xdcb97, 0x11cb97, 0x15cb97, 0x19cb97, 0x1dcb97,
    0x1d397, 0x5d397, 0x9d397, 0xdd397, 0x11d397, 0x15d397, 0x19d397, 0x1dd397, 0x1db97, 0x5db97,
    0x9db97, 0xddb97, 0x11db97, 0x15db97, 0x19db97, 0x1ddb97, 0x1e397, 0x5e397, 0x9e397, 0xde397,
    0x11e397, 0x15e397, 0x19e397, 0x1de397, 0x1eb97, 0x5eb97, 0x9eb97, 0xdeb97, 0x11eb97, 0x15eb97,
    0x19eb97, 0x1deb97, 0x1f397, 0x5f397, 0x9f397, 0xdf397, 0x11f397, 0x15f397, 0x19f397, 0x1df397,
    0x1fb97, 0x5fb97, 0x9fb97, 0xdfb97, 0x11fb97, 0x15fb97, 0x19fb97, 0x1dfb97, 0x1c3a7, 0x5c3a7,
    0x9c3a7, 0xdc3a7, 0x11c3a7, 0x15c3a7, 0x19c3a7, 0x1dc3a7, 0x1cba7, 0x5cba7, 0x9cba7, 0xdcba7,
    0x11cba7, 0x15cba7, 0x19cba7, 0x1dcba7, 0x1d3a7, 0x5d3a7, 0x9d3a7, 0xdd3a7, 0x11d3a7, 0x15d3a7,
    0x19d3a7, 0x1dd3a7, 0x1dba7, 0x5dba7, 0x9dba7, 0xddba7, 0x11dba7, 0x15dba7, 0x19dba7, 0x1ddba7,
    0x1e3a7, 0x5e3a7, 0x9e3a7, 0xde3a7, 0x11e3a7, 0x15e3a7, 0x19e3a7, 0x1de3a7, 0x1eba7, 0x5eba7,
    0x9eba7, 0xdeba7, 0x11eba7, 0x15eba7, 0x19eba7, 0x1deba7, 0x1f3a7, 0x5f3a7, 0x9f3a7, 0xdf3a7,
    0x11f3a7, 0x15f3a7, 0x19f3a7, 0x1df3a7, 0x1fba7, 0x5fba7, 0x9fba7, 0xdfba7, 0x11fba7, 0x15fba7,
    0x19fba7, 0x1dfba7, 0x1c3b7, 0x5c3b7, 0x9c3b7, 0xdc3b7, 0x11c3b7, 0x15c3b7, 0x19c3b7, 0x1dc3b7,
    0x1cbb7, 0x5cbb7, 0x9cbb7, 0xdcbb7, 0x11cbb7, 0x15cbb7, 0x19cbb7, 0x1dcbb7, 0x1d3b7, 0x5d3b7,
    0x9d3b7, 0xdd3b7, 0x11d3b7, 0x15d3b7, 0x19d3b7, 0x1dd3b7, 0x1dbb7, 0x5dbb7, 0x9dbb7, 0xddbb7,
    0x11dbb7, 0x15dbb7, 0x19dbb7, 0x1ddbb7, 0x1e3b7, 0x5e3b7, 0x9e3b7, 0xde3b7, 0x11e3b7, 0x15e3b7,
    0x19e3b7, 0x1de3b7, 0x1ebb7, 0x5ebb7, 0x9ebb7, 0xdebb7, 0x11ebb7, 0x15ebb7, 0x19ebb7, 0x1debb7,
    0x1f3b7, 0x5f3b7, 0x9f3b7, 0xdf3b7, 0x11f3b7, 0x15f3b7, 0x19f3b7, 0x1df3b7, 0x1fbb7, 0x5fbb7,
    0x9fbb7, 0xdfbb7, 0x11fbb7, 0x15fbb7, 0x19fbb7, 0x1dfbb7, 0x1c3c7, 0x5c3c7, 0x9c3c7, 0xdc3c7,
    0x11c3c7, 0x15c3c7, 0x19c3c7, 0x1dc3c7, 0x1cbc7, 0x5cbc7, 0x9cbc7, 0xdcbc7, 0x11cbc7, 0x15cbc7,
    0x19cbc7, 0x1dcbc7, 0x1d3c7, 0x5d3c7, 0x9d3c7, 0xdd3c7, 0x11d3c7, 0x15d3c7, 0x19d3c7, 0x1dd3c7,
    0x1dbc7, 0x5dbc7, 0x9dbc7, 0xddbc7, 0x11dbc7, 0x15dbc7, 0x19dbc7, 0x1ddbc7, 0x1e3c7, 0x5e3c7,
    0x9e3c7, 0xde3c7, 0x11e3c7, 0x15e3c7, 0x19e3c7, 0x1de3c7, 0x1ebc7, 0x5ebc7, 0x9ebc7, 0xdebc7,
    0x11ebc7, 0x15ebc7, 0x19ebc7, 0x1debc7, 0x1f3c7, 0x5f3c7, 0x9f3c7, 0xdf3c7, 0x11f3c7, 0x15f3c7,
    0x19f3c7, 0x1df3c7, 0x1fbc7, 0x5fbc7, 0x9fbc7, 0xdfbc7, 0x11fbc7, 0x15fbc7, 0x19fbc7, 0x1dfbc7,
    0x1c3d7, 0x5c3d7, 0x9c3d7, 0xdc3d7, 0x11c3d7, 0x15c3d7, 0x19c3d7, 0x1dc3d7, 0x1cbd7, 0x5cbd7,
    0x9cbd7, 0xdcbd7, 0x11cbd7, 0x15cbd7, 0x19cbd7, 0x1dcbd7, 0x1d3d7, 0x5d3d7, 0x9d3d7, 0xdd3d7,
    0x11d3d7, 0x15d3d7, 0x19d3d7, 0x1dd3d7, 0x1dbd7, 0x5dbd7, 0x9dbd7, 0xddbd7, 0x11dbd7, 0x15dbd7,
    0x19dbd7, 0x1ddbd7, 0x1e3d7, 0x5e3d7, 0x9e3d7, 0xde3d7, 0x11e3d7, 0x15e3d7, 0x19e3d7, 0x1de3d7,
    0x1ebd7, 0x5ebd7, 0x9ebd7, 0xdebd7, 0x11ebd7, 0x15ebd7, 0x19ebd7, 0x1debd7, 0x1f3d7, 0x5f3d7,
    0x9f3d7, 0xdf3d7, 0x11f3d7, 0x15f3d7, 0x19f3d7, 0x1df3d7, 0x1fbd7, 0x5fbd7, 0x9fbd7, 0xdfbd7,
    0x11fbd7, 0x15fbd7, 0x19fbd7, 0x1dfbd7, 0x1c3e7, 0x5c3e7, 0x9c3e7, 0xdc3e7, 0x11c3e7, 0x15c3e7,
    0x19c3e7, 0x1dc3e7, 0x1cbe7, 0x5cbe7, 0x9cbe7, 0xdcbe7, 0x11cbe7, 0x15cbe7, 0x19cbe7, 0x1dcbe7,
    0x1d3e7, 0x5d3e7, 0x9d3e7, 0xdd3e7, 0x11d3e7, 0x15d3e7, 0x19d3e7, 0x1dd3e7, 0x1dbe7, 0x5dbe7,
    0x9dbe7, 0xddbe7, 0x11dbe7, 0x15dbe7, 0x19dbe7, 0x1ddbe7, 0x1e3e7, 0x5e3e7, 0x9e3e7, 0xde3e7,
    0x11e3e7, 0x15e3e7, 0x19e3e7, 0x1de3e7, 0x1ebe7, 0x5ebe7, 0x9ebe7, 0xdebe7, 0x11ebe7, 0x15ebe7,
    0x19ebe7, 0x1debe7, 0x1f3e7, 0x5f3e7, 0x9f3e7, 0xdf3e7, 0x11f3e7, 0x15f3e7, 0x19f3e7, 0x1df3e7,
    0x1fbe7, 0x5fbe7, 0x9fbe7, 0xdfbe7, 0x11fbe7, 0x15fbe7, 0x19fbe7, 0x1dfbe7, 0x1c3f7, 0x5c3f7,
    0x9c3f7, 0xdc3f7, 0x11c3f7, 0x15c3f7, 0x19c3f7, 0x1dc3f7, 0x1cbf7, 0x5cbf7, 0x9cbf7, 0xdcbf7,
    0x11cbf7, 0x15cbf7, 0x19cbf7, 0x1dcbf7, 0x1d3f7, 0x5d3f7, 0x9d3f7, 0xdd3f7, 0x11d3f7, 0x15d3f7,
    0x19d3f7, 0x1dd3f7, 0x1dbf7, 0x5dbf7, 0x9dbf7, 0xddbf7, 0x11dbf7, 0x15dbf7, 0x19dbf7, 0x1ddbf7,
    0x1e3f7, 0x5e3f7, 0x9e3f7, 0xde3f7, 0x11e3f7, 0x15e3f7, 0x19e3f7, 0x1de3f7, 0x1ebf7, 0x5ebf7,
    0x9ebf7, 0xdebf7, 0x11ebf7, 0x15ebf7, 0x19ebf7, 0x1debf7, 0x1f3f7, 0x5f3f7, 0x9f3f7, 0xdf3f7,
    0x11f3f7, 0x15f3f7, 0x19f3f7, 0x1df3f7, 0x1fbf7, 0x5fbf7, 0x9fbf7, 0xdfbf7, 0x11fbf7, 0x15fbf7,
    0x19fbf7, 0x1dfbf7, 0xe1c387, 0x2e1c387, 0x4e1c387, 0x6e1c387, 0x8e1c387, 0xae1c387, 0xce1c387,
    0xee1c387, 0xe5c387, 0x2e5c387, 0x4e5c387, 0x6e5c387, 0x8e5c387, 0xae5c387, 0xce5c387,
    0xee5c387, 0xe9c387, 0x2e9c387, 0x4e9c387, 0x6e9c387, 0x8e9c387, 0xae9c387, 0xce9c387,
    0xee9c387, 0xedc387, 0x2edc387, 0x4edc387, 0x6edc387, 0x8edc387, 0xaedc387, 0xcedc387,
    0xeedc387, 0xf1c387, 0x2f1c387, 0x4f1c387, 0x6f1c387, 0x8f1c387, 0xaf1c387, 0xcf1c387,
    0xef1c387, 0xf5c387, 0x2f5c387, 0x4f5c387, 0x6f5c387, 0x8f5c387, 0xaf5c387, 0xcf5c387,
    0xef5c387, 0xf9c387, 0x2f9c387, 0x4f9c387, 0x6f9c387, 0x8f9c387, 0xaf9c387, 0xcf9c387,
    0xef9c387, 0xfdc387, 0x2fdc387, 0x4fdc387, 0x6fdc387, 0x8fdc387, 0xafdc387, 0xcfdc387,
    0xefdc387, 0xe1cb87, 0x2e1cb87, 0x4e1cb87, 0x6e1cb87, 0x8e1cb87, 0xae1cb87, 0xce1cb87,
    0xee1cb87, 0xe5cb87, 0x2e5cb87, 0x4e5cb87, 0x6e5cb87, 0x8e5cb87, 0xae5cb87, 0xce5cb87,
    0xee5cb87, 0xe9cb87, 0x2e9cb87, 0x4e9cb87, 0x6e9cb87, 0x8e9cb87, 0xae9cb87, 0xce9cb87,
    0xee9cb87, 0xedcb87, 0x2edcb87, 0x4edcb87, 0x6edcb87, 0x8edcb87, 0xaedcb87, 0xcedcb87,
    0xeedcb87, 0xf1cb87, 0x2f1cb87, 0x4f1cb87, 0x6f1cb87, 0x8f1cb87, 0xaf1cb87, 0xcf1cb87,
    0xef1cb87, 0xf5cb87, 0x2f5cb87, 0x4f5cb87, 0x6f5cb87, 0x8f5cb87, 0xaf5cb87, 0xcf5cb87,
    0xef5cb87, 0xf9cb87, 0x2f9cb87, 0x4f9cb87, 0x6f9cb87, 0x8f9cb87,
];

static kZeroRepsDepth: [u32; 704] = [
    0, 4, 8, 7, 7, 7, 7, 7, 7, 7, 7, 11, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14,
    14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14,
    14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14,
    14, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
    28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
    28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
    28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
    28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
    28, 28, 28, 28, 28, 28,
];

static kNonZeroRepsBits: [u64; 704] = [
    0xb, 0x1b, 0x2b, 0x3b, 0x2cb, 0x6cb, 0xacb, 0xecb, 0x2db, 0x6db, 0xadb, 0xedb, 0x2eb, 0x6eb,
    0xaeb, 0xeeb, 0x2fb, 0x6fb, 0xafb, 0xefb, 0xb2cb, 0x1b2cb, 0x2b2cb, 0x3b2cb, 0xb6cb, 0x1b6cb,
    0x2b6cb, 0x3b6cb, 0xbacb, 0x1bacb, 0x2bacb, 0x3bacb, 0xbecb, 0x1becb, 0x2becb, 0x3becb, 0xb2db,
    0x1b2db, 0x2b2db, 0x3b2db, 0xb6db, 0x1b6db, 0x2b6db, 0x3b6db, 0xbadb, 0x1badb, 0x2badb,
    0x3badb, 0xbedb, 0x1bedb, 0x2bedb, 0x3bedb, 0xb2eb, 0x1b2eb, 0x2b2eb, 0x3b2eb, 0xb6eb, 0x1b6eb,
    0x2b6eb, 0x3b6eb, 0xbaeb, 0x1baeb, 0x2baeb, 0x3baeb, 0xbeeb, 0x1beeb, 0x2beeb, 0x3beeb, 0xb2fb,
    0x1b2fb, 0x2b2fb, 0x3b2fb, 0xb6fb, 0x1b6fb, 0x2b6fb, 0x3b6fb, 0xbafb, 0x1bafb, 0x2bafb,
    0x3bafb, 0xbefb, 0x1befb, 0x2befb, 0x3befb, 0x2cb2cb, 0x6cb2cb, 0xacb2cb, 0xecb2cb, 0x2db2cb,
    0x6db2cb, 0xadb2cb, 0xedb2cb, 0x2eb2cb, 0x6eb2cb, 0xaeb2cb, 0xeeb2cb, 0x2fb2cb, 0x6fb2cb,
    0xafb2cb, 0xefb2cb, 0x2cb6cb, 0x6cb6cb, 0xacb6cb, 0xecb6cb, 0x2db6cb, 0x6db6cb, 0xadb6cb,
    0xedb6cb, 0x2eb6cb, 0x6eb6cb, 0xaeb6cb, 0xeeb6cb, 0x2fb6cb, 0x6fb6cb, 0xafb6cb, 0xefb6cb,
    0x2cbacb, 0x6cbacb, 0xacbacb, 0xecbacb, 0x2dbacb, 0x6dbacb, 0xadbacb, 0xedbacb, 0x2ebacb,
    0x6ebacb, 0xaebacb, 0xeebacb, 0x2fbacb, 0x6fbacb, 0xafbacb, 0xefbacb, 0x2cbecb, 0x6cbecb,
    0xacbecb, 0xecbecb, 0x2dbecb, 0x6dbecb, 0xadbecb, 0xedbecb, 0x2ebecb, 0x6ebecb, 0xaebecb,
    0xeebecb, 0x2fbecb, 0x6fbecb, 0xafbecb, 0xefbecb, 0x2cb2db, 0x6cb2db, 0xacb2db, 0xecb2db,
    0x2db2db, 0x6db2db, 0xadb2db, 0xedb2db, 0x2eb2db, 0x6eb2db, 0xaeb2db, 0xeeb2db, 0x2fb2db,
    0x6fb2db, 0xafb2db, 0xefb2db, 0x2cb6db, 0x6cb6db, 0xacb6db, 0xecb6db, 0x2db6db, 0x6db6db,
    0xadb6db, 0xedb6db, 0x2eb6db, 0x6eb6db, 0xaeb6db, 0xeeb6db, 0x2fb6db, 0x6fb6db, 0xafb6db,
    0xefb6db, 0x2cbadb, 0x6cbadb, 0xacbadb, 0xecbadb, 0x2dbadb, 0x6dbadb, 0xadbadb, 0xedbadb,
    0x2ebadb, 0x6ebadb, 0xaebadb, 0xeebadb, 0x2fbadb, 0x6fbadb, 0xafbadb, 0xefbadb, 0x2cbedb,
    0x6cbedb, 0xacbedb, 0xecbedb, 0x2dbedb, 0x6dbedb, 0xadbedb, 0xedbedb, 0x2ebedb, 0x6ebedb,
    0xaebedb, 0xeebedb, 0x2fbedb, 0x6fbedb, 0xafbedb, 0xefbedb, 0x2cb2eb, 0x6cb2eb, 0xacb2eb,
    0xecb2eb, 0x2db2eb, 0x6db2eb, 0xadb2eb, 0xedb2eb, 0x2eb2eb, 0x6eb2eb, 0xaeb2eb, 0xeeb2eb,
    0x2fb2eb, 0x6fb2eb, 0xafb2eb, 0xefb2eb, 0x2cb6eb, 0x6cb6eb, 0xacb6eb, 0xecb6eb, 0x2db6eb,
    0x6db6eb, 0xadb6eb, 0xedb6eb, 0x2eb6eb, 0x6eb6eb, 0xaeb6eb, 0xeeb6eb, 0x2fb6eb, 0x6fb6eb,
    0xafb6eb, 0xefb6eb, 0x2cbaeb, 0x6cbaeb, 0xacbaeb, 0xecbaeb, 0x2dbaeb, 0x6dbaeb, 0xadbaeb,
    0xedbaeb, 0x2ebaeb, 0x6ebaeb, 0xaebaeb, 0xeebaeb, 0x2fbaeb, 0x6fbaeb, 0xafbaeb, 0xefbaeb,
    0x2cbeeb, 0x6cbeeb, 0xacbeeb, 0xecbeeb, 0x2dbeeb, 0x6dbeeb, 0xadbeeb, 0xedbeeb, 0x2ebeeb,
    0x6ebeeb, 0xaebeeb, 0xeebeeb, 0x2fbeeb, 0x6fbeeb, 0xafbeeb, 0xefbeeb, 0x2cb2fb, 0x6cb2fb,
    0xacb2fb, 0xecb2fb, 0x2db2fb, 0x6db2fb, 0xadb2fb, 0xedb2fb, 0x2eb2fb, 0x6eb2fb, 0xaeb2fb,
    0xeeb2fb, 0x2fb2fb, 0x6fb2fb, 0xafb2fb, 0xefb2fb, 0x2cb6fb, 0x6cb6fb, 0xacb6fb, 0xecb6fb,
    0x2db6fb, 0x6db6fb, 0xadb6fb, 0xedb6fb, 0x2eb6fb, 0x6eb6fb, 0xaeb6fb, 0xeeb6fb, 0x2fb6fb,
    0x6fb6fb, 0xafb6fb, 0xefb6fb, 0x2cbafb, 0x6cbafb, 0xacbafb, 0xecbafb, 0x2dbafb, 0x6dbafb,
    0xadbafb, 0xedbafb, 0x2ebafb, 0x6ebafb, 0xaebafb, 0xeebafb, 0x2fbafb, 0x6fbafb, 0xafbafb,
    0xefbafb, 0x2cbefb, 0x6cbefb, 0xacbefb, 0xecbefb, 0x2dbefb, 0x6dbefb, 0xadbefb, 0xedbefb,
    0x2ebefb, 0x6ebefb, 0xaebefb, 0xeebefb, 0x2fbefb, 0x6fbefb, 0xafbefb, 0xefbefb, 0xb2cb2cb,
    0x1b2cb2cb, 0x2b2cb2cb, 0x3b2cb2cb, 0xb6cb2cb, 0x1b6cb2cb, 0x2b6cb2cb, 0x3b6cb2cb, 0xbacb2cb,
    0x1bacb2cb, 0x2bacb2cb, 0x3bacb2cb, 0xbecb2cb, 0x1becb2cb, 0x2becb2cb, 0x3becb2cb, 0xb2db2cb,
    0x1b2db2cb, 0x2b2db2cb, 0x3b2db2cb, 0xb6db2cb, 0x1b6db2cb, 0x2b6db2cb, 0x3b6db2cb, 0xbadb2cb,
    0x1badb2cb, 0x2badb2cb, 0x3badb2cb, 0xbedb2cb, 0x1bedb2cb, 0x2bedb2cb, 0x3bedb2cb, 0xb2eb2cb,
    0x1b2eb2cb, 0x2b2eb2cb, 0x3b2eb2cb, 0xb6eb2cb, 0x1b6eb2cb, 0x2b6eb2cb, 0x3b6eb2cb, 0xbaeb2cb,
    0x1baeb2cb, 0x2baeb2cb, 0x3baeb2cb, 0xbeeb2cb, 0x1beeb2cb, 0x2beeb2cb, 0x3beeb2cb, 0xb2fb2cb,
    0x1b2fb2cb, 0x2b2fb2cb, 0x3b2fb2cb, 0xb6fb2cb, 0x1b6fb2cb, 0x2b6fb2cb, 0x3b6fb2cb, 0xbafb2cb,
    0x1bafb2cb, 0x2bafb2cb, 0x3bafb2cb, 0xbefb2cb, 0x1befb2cb, 0x2befb2cb, 0x3befb2cb, 0xb2cb6cb,
    0x1b2cb6cb, 0x2b2cb6cb, 0x3b2cb6cb, 0xb6cb6cb, 0x1b6cb6cb, 0x2b6cb6cb, 0x3b6cb6cb, 0xbacb6cb,
    0x1bacb6cb, 0x2bacb6cb, 0x3bacb6cb, 0xbecb6cb, 0x1becb6cb, 0x2becb6cb, 0x3becb6cb, 0xb2db6cb,
    0x1b2db6cb, 0x2b2db6cb, 0x3b2db6cb, 0xb6db6cb, 0x1b6db6cb, 0x2b6db6cb, 0x3b6db6cb, 0xbadb6cb,
    0x1badb6cb, 0x2badb6cb, 0x3badb6cb, 0xbedb6cb, 0x1bedb6cb, 0x2bedb6cb, 0x3bedb6cb, 0xb2eb6cb,
    0x1b2eb6cb, 0x2b2eb6cb, 0x3b2eb6cb, 0xb6eb6cb, 0x1b6eb6cb, 0x2b6eb6cb, 0x3b6eb6cb, 0xbaeb6cb,
    0x1baeb6cb, 0x2baeb6cb, 0x3baeb6cb, 0xbeeb6cb, 0x1beeb6cb, 0x2beeb6cb, 0x3beeb6cb, 0xb2fb6cb,
    0x1b2fb6cb, 0x2b2fb6cb, 0x3b2fb6cb, 0xb6fb6cb, 0x1b6fb6cb, 0x2b6fb6cb, 0x3b6fb6cb, 0xbafb6cb,
    0x1bafb6cb, 0x2bafb6cb, 0x3bafb6cb, 0xbefb6cb, 0x1befb6cb, 0x2befb6cb, 0x3befb6cb, 0xb2cbacb,
    0x1b2cbacb, 0x2b2cbacb, 0x3b2cbacb, 0xb6cbacb, 0x1b6cbacb, 0x2b6cbacb, 0x3b6cbacb, 0xbacbacb,
    0x1bacbacb, 0x2bacbacb, 0x3bacbacb, 0xbecbacb, 0x1becbacb, 0x2becbacb, 0x3becbacb, 0xb2dbacb,
    0x1b2dbacb, 0x2b2dbacb, 0x3b2dbacb, 0xb6dbacb, 0x1b6dbacb, 0x2b6dbacb, 0x3b6dbacb, 0xbadbacb,
    0x1badbacb, 0x2badbacb, 0x3badbacb, 0xbedbacb, 0x1bedbacb, 0x2bedbacb, 0x3bedbacb, 0xb2ebacb,
    0x1b2ebacb, 0x2b2ebacb, 0x3b2ebacb, 0xb6ebacb, 0x1b6ebacb, 0x2b6ebacb, 0x3b6ebacb, 0xbaebacb,
    0x1baebacb, 0x2baebacb, 0x3baebacb, 0xbeebacb, 0x1beebacb, 0x2beebacb, 0x3beebacb, 0xb2fbacb,
    0x1b2fbacb, 0x2b2fbacb, 0x3b2fbacb, 0xb6fbacb, 0x1b6fbacb, 0x2b6fbacb, 0x3b6fbacb, 0xbafbacb,
    0x1bafbacb, 0x2bafbacb, 0x3bafbacb, 0xbefbacb, 0x1befbacb, 0x2befbacb, 0x3befbacb, 0xb2cbecb,
    0x1b2cbecb, 0x2b2cbecb, 0x3b2cbecb, 0xb6cbecb, 0x1b6cbecb, 0x2b6cbecb, 0x3b6cbecb, 0xbacbecb,
    0x1bacbecb, 0x2bacbecb, 0x3bacbecb, 0xbecbecb, 0x1becbecb, 0x2becbecb, 0x3becbecb, 0xb2dbecb,
    0x1b2dbecb, 0x2b2dbecb, 0x3b2dbecb, 0xb6dbecb, 0x1b6dbecb, 0x2b6dbecb, 0x3b6dbecb, 0xbadbecb,
    0x1badbecb, 0x2badbecb, 0x3badbecb, 0xbedbecb, 0x1bedbecb, 0x2bedbecb, 0x3bedbecb, 0xb2ebecb,
    0x1b2ebecb, 0x2b2ebecb, 0x3b2ebecb, 0xb6ebecb, 0x1b6ebecb, 0x2b6ebecb, 0x3b6ebecb, 0xbaebecb,
    0x1baebecb, 0x2baebecb, 0x3baebecb, 0xbeebecb, 0x1beebecb, 0x2beebecb, 0x3beebecb, 0xb2fbecb,
    0x1b2fbecb, 0x2b2fbecb, 0x3b2fbecb, 0xb6fbecb, 0x1b6fbecb, 0x2b6fbecb, 0x3b6fbecb, 0xbafbecb,
    0x1bafbecb, 0x2bafbecb, 0x3bafbecb, 0xbefbecb, 0x1befbecb, 0x2befbecb, 0x3befbecb, 0xb2cb2db,
    0x1b2cb2db, 0x2b2cb2db, 0x3b2cb2db, 0xb6cb2db, 0x1b6cb2db, 0x2b6cb2db, 0x3b6cb2db, 0xbacb2db,
    0x1bacb2db, 0x2bacb2db, 0x3bacb2db, 0xbecb2db, 0x1becb2db, 0x2becb2db, 0x3becb2db, 0xb2db2db,
    0x1b2db2db, 0x2b2db2db, 0x3b2db2db, 0xb6db2db, 0x1b6db2db, 0x2b6db2db, 0x3b6db2db, 0xbadb2db,
    0x1badb2db, 0x2badb2db, 0x3badb2db, 0xbedb2db, 0x1bedb2db, 0x2bedb2db, 0x3bedb2db, 0xb2eb2db,
    0x1b2eb2db, 0x2b2eb2db, 0x3b2eb2db, 0xb6eb2db, 0x1b6eb2db, 0x2b6eb2db, 0x3b6eb2db, 0xbaeb2db,
    0x1baeb2db, 0x2baeb2db, 0x3baeb2db, 0xbeeb2db, 0x1beeb2db, 0x2beeb2db, 0x3beeb2db, 0xb2fb2db,
    0x1b2fb2db, 0x2b2fb2db, 0x3b2fb2db, 0xb6fb2db, 0x1b6fb2db, 0x2b6fb2db, 0x3b6fb2db, 0xbafb2db,
    0x1bafb2db, 0x2bafb2db, 0x3bafb2db, 0xbefb2db, 0x1befb2db, 0x2befb2db, 0x3befb2db, 0xb2cb6db,
    0x1b2cb6db, 0x2b2cb6db, 0x3b2cb6db, 0xb6cb6db, 0x1b6cb6db, 0x2b6cb6db, 0x3b6cb6db, 0xbacb6db,
    0x1bacb6db, 0x2bacb6db, 0x3bacb6db, 0xbecb6db, 0x1becb6db, 0x2becb6db, 0x3becb6db, 0xb2db6db,
    0x1b2db6db, 0x2b2db6db, 0x3b2db6db, 0xb6db6db, 0x1b6db6db, 0x2b6db6db, 0x3b6db6db, 0xbadb6db,
    0x1badb6db, 0x2badb6db, 0x3badb6db, 0xbedb6db, 0x1bedb6db, 0x2bedb6db, 0x3bedb6db, 0xb2eb6db,
    0x1b2eb6db, 0x2b2eb6db, 0x3b2eb6db, 0xb6eb6db, 0x1b6eb6db, 0x2b6eb6db, 0x3b6eb6db, 0xbaeb6db,
    0x1baeb6db, 0x2baeb6db, 0x3baeb6db,
];

static kNonZeroRepsDepth: [u32; 704] = [
    6, 6, 6, 6, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 18, 18, 18, 18, 18,
    18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18,
    18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18,
    18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    24, 24, 24, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30,
];

// ── Store code-length code (static) ──

fn StoreStaticCodeLengthCode(pos: &mut usize, storage: &mut [u8]) {
    BrotliWriteBits(40, 0xff_5555_5554, pos, storage);
}

// ── Store code-length Huffman tree structure ──

fn BrotliStoreHuffmanTreeOfHuffmanTreeToBitMask(
    num_codes: i32,
    code_length_bitdepth: &[u8],
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    static kStorageOrder: [u8; 18] = [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];
    static kSymbols: [u8; 6] = [0, 7, 3, 2, 1, 15];
    static kBitLengths: [u8; 6] = [2, 4, 3, 2, 2, 4];
    let mut skip_some: u64 = 0;
    let mut codes_to_store: u64 = 18;
    if num_codes > 1 {
        while codes_to_store > 0 {
            if code_length_bitdepth[kStorageOrder[(codes_to_store - 1) as usize] as usize] != 0 {
                break;
            }
            codes_to_store -= 1;
        }
    }
    if code_length_bitdepth[kStorageOrder[0] as usize] == 0
        && code_length_bitdepth[kStorageOrder[1] as usize] == 0
    {
        skip_some = 2;
        if code_length_bitdepth[kStorageOrder[2] as usize] == 0 {
            skip_some = 3;
        }
    }
    BrotliWriteBits(2, skip_some, storage_ix, storage);
    for i in skip_some..codes_to_store {
        let l = code_length_bitdepth[kStorageOrder[i as usize] as usize] as usize;
        BrotliWriteBits(
            kBitLengths[l] as usize,
            u64::from(kSymbols[l]),
            storage_ix,
            storage,
        );
    }
}

fn BrotliStoreHuffmanTreeToBitMask(
    huffman_tree_size: usize,
    huffman_tree: &[u8],
    huffman_tree_extra_bits: &[u8],
    code_length_bitdepth: &[u8],
    code_length_bitdepth_symbols: &[u16],
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    for i in 0..huffman_tree_size {
        let ix = huffman_tree[i] as usize;
        BrotliWriteBits(
            code_length_bitdepth[ix] as usize,
            u64::from(code_length_bitdepth_symbols[ix]),
            storage_ix,
            storage,
        );
        if ix == 16 {
            BrotliWriteBits(
                2,
                u64::from(huffman_tree_extra_bits[i]),
                storage_ix,
                storage,
            );
        } else if ix == 17 {
            BrotliWriteBits(
                3,
                u64::from(huffman_tree_extra_bits[i]),
                storage_ix,
                storage,
            );
        }
    }
}

pub(crate) fn BrotliStoreHuffmanTree(
    depths: &[u8],
    num: usize,
    tree: &mut [HuffmanTree],
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    let mut huffman_tree = [0u8; 704];
    let mut huffman_tree_extra_bits = [0u8; 704];
    let mut huffman_tree_size = 0usize;
    let mut code_length_bitdepth = [0u8; 19];
    let mut code_length_bitdepth_symbols = [0u16; 19];
    let mut huffman_tree_histogram = [0u32; 19];
    BrotliWriteHuffmanTree(
        depths,
        num,
        &mut huffman_tree,
        &mut huffman_tree_extra_bits,
        &mut huffman_tree_size,
    );

    for i in 0..huffman_tree_size {
        huffman_tree_histogram[huffman_tree[i] as usize] += 1;
    }
    let mut num_codes: i32 = 0;
    let mut code: usize = 0;
    let mut i = 0;
    while i < 18 {
        if huffman_tree_histogram[i] != 0 {
            if num_codes == 0 {
                code = i;
                num_codes = 1;
            } else if num_codes == 1 {
                num_codes = 2;
                break;
            }
        }
        i += 1;
    }
    BrotliCreateHuffmanTree(
        &huffman_tree_histogram,
        18,
        5,
        tree,
        &mut code_length_bitdepth,
    );
    BrotliConvertBitDepthsToSymbols(&code_length_bitdepth, 18, &mut code_length_bitdepth_symbols);
    BrotliStoreHuffmanTreeOfHuffmanTreeToBitMask(
        num_codes,
        &code_length_bitdepth,
        storage_ix,
        storage,
    );
    if num_codes == 1 {
        code_length_bitdepth[code] = 0;
    }
    BrotliStoreHuffmanTreeToBitMask(
        huffman_tree_size,
        &huffman_tree,
        &huffman_tree_extra_bits,
        &code_length_bitdepth,
        &code_length_bitdepth_symbols,
        storage_ix,
        storage,
    );
}

// ── Build + store Huffman tree (fast variant with simple-form) ──

pub(crate) fn BrotliBuildAndStoreHuffmanTreeFast(
    histogram: &[u32],
    histogram_total: usize,
    max_bits: u8,
    depth: &mut [u8],
    bits: &mut [u16],
    storage_ix: &mut usize,
    storage: &mut [u8],
    tree_scratch: &mut [HuffmanTree],
) {
    let mut count: usize = 0;
    let mut symbols: [u64; 4] = [0; 4];
    let mut length: usize = 0;
    let mut total = histogram_total;
    while total != 0 {
        if histogram[length] != 0 {
            if count < 4 {
                symbols[count] = length as u64;
            }
            count += 1;
            total = total.wrapping_sub(histogram[length] as usize);
        }
        length += 1;
    }
    if count <= 1 {
        BrotliWriteBits(4, 1, storage_ix, storage);
        BrotliWriteBits(max_bits as usize, symbols[0], storage_ix, storage);
        depth[symbols[0] as usize] = 0;
        bits[symbols[0] as usize] = 0;
        return;
    }
    for d in &mut depth[..length] {
        *d = 0;
    }
    let sentinel = HuffmanTree::new(u32::MAX, -1, -1);
    let mut count_limit: u32 = 1;
    loop {
        let mut node_index: usize = 0;
        let mut l = length;
        while l != 0 {
            l -= 1;
            if histogram[l] != 0 {
                let c = std::cmp::max(histogram[l], count_limit);
                tree_scratch[node_index] = HuffmanTree::new(c, -1, l as i16);
                node_index += 1;
            }
        }
        let n: usize = node_index;
        if n == 1 {
            depth[tree_scratch[0].index_right_or_value_ as usize] = 1u8;
            break;
        }
        let mut i: usize = 0;
        let mut j: usize = n + 1;
        SortHuffmanTreeItems(tree_scratch, n, SimpleSort {});
        tree_scratch[n] = sentinel;
        tree_scratch[n + 1] = sentinel;
        let mut k = n - 1;
        while k != 0 {
            let left = if tree_scratch[i].total_count_ <= tree_scratch[j].total_count_ {
                let l = i;
                i += 1;
                l
            } else {
                let l = j;
                j += 1;
                l
            };
            let right = if tree_scratch[i].total_count_ <= tree_scratch[j].total_count_ {
                let r = i;
                i += 1;
                r
            } else {
                let r = j;
                j += 1;
                r
            };
            let j_end = 2 * n - k;
            tree_scratch[j_end] = HuffmanTree::new(
                tree_scratch[left]
                    .total_count_
                    .wrapping_add(tree_scratch[right].total_count_),
                left as i16,
                right as i16,
            );
            tree_scratch[j_end + 1] = sentinel;
            k -= 1;
        }
        if BrotliSetDepth((2 * n - 1) as i32, tree_scratch, depth, 14) {
            break;
        }
        count_limit = count_limit.wrapping_mul(2);
    }
    BrotliConvertBitDepthsToSymbols(depth, length, bits);
    if count <= 4 {
        BrotliWriteBits(2, 1, storage_ix, storage);
        BrotliWriteBits(2, (count - 1) as u64, storage_ix, storage);
        for i in 0..count {
            for j in i + 1..count {
                if depth[symbols[j] as usize] < depth[symbols[i] as usize] {
                    symbols.swap(j, i);
                }
            }
        }
        if count == 2 {
            BrotliWriteBits(max_bits as usize, symbols[0], storage_ix, storage);
            BrotliWriteBits(max_bits as usize, symbols[1], storage_ix, storage);
        } else if count == 3 {
            BrotliWriteBits(max_bits as usize, symbols[0], storage_ix, storage);
            BrotliWriteBits(max_bits as usize, symbols[1], storage_ix, storage);
            BrotliWriteBits(max_bits as usize, symbols[2], storage_ix, storage);
        } else {
            BrotliWriteBits(max_bits as usize, symbols[0], storage_ix, storage);
            BrotliWriteBits(max_bits as usize, symbols[1], storage_ix, storage);
            BrotliWriteBits(max_bits as usize, symbols[2], storage_ix, storage);
            BrotliWriteBits(max_bits as usize, symbols[3], storage_ix, storage);
            BrotliWriteBits(
                1,
                u64::from(depth[symbols[0] as usize] == 1),
                storage_ix,
                storage,
            );
        }
    } else {
        let mut previous_value: u8 = 8;
        StoreStaticCodeLengthCode(storage_ix, storage);
        let mut i: usize = 0;
        while i < length {
            let value = depth[i];
            let mut reps: usize = 1;
            let mut k = i + 1;
            while k < length && depth[k] == value {
                reps += 1;
                k += 1;
            }
            i += reps;
            if value == 0 {
                BrotliWriteBits(
                    kZeroRepsDepth[reps] as usize,
                    kZeroRepsBits[reps],
                    storage_ix,
                    storage,
                );
            } else {
                if previous_value != value {
                    BrotliWriteBits(
                        kCodeLengthDepth[value as usize] as usize,
                        u64::from(kCodeLengthBits[value as usize]),
                        storage_ix,
                        storage,
                    );
                    reps -= 1;
                }
                if reps < 3 {
                    while reps != 0 {
                        reps -= 1;
                        BrotliWriteBits(
                            kCodeLengthDepth[value as usize] as usize,
                            u64::from(kCodeLengthBits[value as usize]),
                            storage_ix,
                            storage,
                        );
                    }
                } else {
                    reps -= 3;
                    BrotliWriteBits(
                        kNonZeroRepsDepth[reps] as usize,
                        kNonZeroRepsBits[reps],
                        storage_ix,
                        storage,
                    );
                }
                previous_value = value;
            }
        }
    }
}

pub(crate) fn store_meta_block_header(
    len: usize,
    is_uncompressed: bool,
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    let mut nibbles: u64 = 6;
    BrotliWriteBits(1, 0, storage_ix, storage);
    if len <= (1u32 << 16) as usize {
        nibbles = 4;
    } else if len <= (1u32 << 20) as usize {
        nibbles = 5;
    }
    BrotliWriteBits(2, nibbles.wrapping_sub(4), storage_ix, storage);
    BrotliWriteBits(
        nibbles.wrapping_mul(4) as usize,
        len.wrapping_sub(1) as u64,
        storage_ix,
        storage,
    );
    BrotliWriteBits(1, u64::from(is_uncompressed), storage_ix, storage);
}
fn EmitInsertLen(insertlen: u32, commands: &mut &mut [u32]) -> usize {
    if insertlen < 6u32 {
        (*commands)[0] = insertlen;
    } else if insertlen < 130u32 {
        let tail: u32 = insertlen.wrapping_sub(2);
        let nbits: u32 = Log2FloorNonZero(u64::from(tail)).wrapping_sub(1);
        let prefix: u32 = tail >> nbits;
        let inscode: u32 = (nbits << 1).wrapping_add(prefix).wrapping_add(2);
        let extra: u32 = tail.wrapping_sub(prefix << nbits);
        (*commands)[0] = inscode | extra << 8;
    } else if insertlen < 2114u32 {
        let tail: u32 = insertlen.wrapping_sub(66);
        let nbits: u32 = Log2FloorNonZero(u64::from(tail));
        let code: u32 = nbits.wrapping_add(10);
        let extra: u32 = tail.wrapping_sub(1u32 << nbits);
        (*commands)[0] = code | extra << 8;
    } else if insertlen < 6210u32 {
        let extra: u32 = insertlen.wrapping_sub(2114);
        (*commands)[0] = 21u32 | extra << 8;
    } else if insertlen < 22594u32 {
        let extra: u32 = insertlen.wrapping_sub(6210);
        (*commands)[0] = 22u32 | extra << 8;
    } else {
        let extra: u32 = insertlen.wrapping_sub(22594);
        (*commands)[0] = 23u32 | extra << 8;
    }
    let remainder = std::mem::take(commands);
    let _ = std::mem::replace(commands, &mut remainder[1..]);
    1
}
fn EmitCopyLen(copylen: usize, commands: &mut &mut [u32]) -> usize {
    if copylen < 10usize {
        (*commands)[0] = copylen.wrapping_add(38) as u32;
    } else if copylen < 134usize {
        let tail: usize = copylen.wrapping_sub(6);
        let nbits: usize = Log2FloorNonZero(tail as u64).wrapping_sub(1) as usize;
        let prefix: usize = tail >> nbits;
        let code: usize = (nbits << 1).wrapping_add(prefix).wrapping_add(44);
        let extra: usize = tail.wrapping_sub(prefix << nbits);
        (*commands)[0] = (code | extra << 8) as u32;
    } else if copylen < 2118usize {
        let tail: usize = copylen.wrapping_sub(70);
        let nbits: usize = Log2FloorNonZero(tail as u64) as usize;
        let code: usize = nbits.wrapping_add(52);
        let extra: usize = tail.wrapping_sub(1usize << nbits);
        (*commands)[0] = (code | extra << 8) as u32;
    } else {
        let extra: usize = copylen.wrapping_sub(2118);
        (*commands)[0] = (63usize | extra << 8) as u32;
    }
    let remainder = std::mem::take(commands);
    let _ = std::mem::replace(commands, &mut remainder[1..]);
    1
}
fn EmitCopyLenLastDistance(copylen: usize, commands: &mut &mut [u32]) -> usize {
    if copylen < 12usize {
        (*commands)[0] = copylen.wrapping_add(20) as u32;
        let remainder = std::mem::take(commands);
        let _ = std::mem::replace(commands, &mut remainder[1..]);
        1
    } else if copylen < 72usize {
        let tail: usize = copylen.wrapping_sub(8);
        let nbits: usize = Log2FloorNonZero(tail as u64).wrapping_sub(1) as usize;
        let prefix: usize = tail >> nbits;
        let code: usize = (nbits << 1).wrapping_add(prefix).wrapping_add(28);
        let extra: usize = tail.wrapping_sub(prefix << nbits);
        (*commands)[0] = (code | extra << 8) as u32;
        let remainder = std::mem::take(commands);
        let _ = std::mem::replace(commands, &mut remainder[1..]);
        1
    } else if copylen < 136usize {
        let tail: usize = copylen.wrapping_sub(8);
        let code: usize = (tail >> 5).wrapping_add(54);
        let extra: usize = tail & 31usize;
        (*commands)[0] = (code | extra << 8) as u32;
        let remainder = std::mem::take(commands);
        let _ = std::mem::replace(commands, &mut remainder[1..]);
        (*commands)[0] = 64u32;
        let remainder2 = std::mem::take(commands);
        let _ = std::mem::replace(commands, &mut remainder2[1..]);
        2
    } else if copylen < 2120usize {
        let tail: usize = copylen.wrapping_sub(72);
        let nbits: usize = Log2FloorNonZero(tail as u64) as usize;
        let code: usize = nbits.wrapping_add(52);
        let extra: usize = tail.wrapping_sub(1usize << nbits);
        (*commands)[0] = (code | extra << 8) as u32;
        let remainder = std::mem::take(commands);
        let _ = std::mem::replace(commands, &mut remainder[1..]);
        (*commands)[0] = 64u32;
        let remainder2 = std::mem::take(commands);
        let _ = std::mem::replace(commands, &mut remainder2[1..]);
        2
    } else {
        let extra: usize = copylen.wrapping_sub(2120);
        (*commands)[0] = (63usize | extra << 8) as u32;
        let remainder = std::mem::take(commands);
        let _ = std::mem::replace(commands, &mut remainder[1..]);
        (*commands)[0] = 64u32;
        let remainder2 = std::mem::take(commands);
        let _ = std::mem::replace(commands, &mut remainder2[1..]);
        2
    }
}
fn EmitDistance(distance: u32, commands: &mut &mut [u32]) -> usize {
    let d: u32 = distance.wrapping_add(3);
    let nbits: u32 = Log2FloorNonZero(u64::from(d)).wrapping_sub(1);
    let prefix: u32 = d >> nbits & 1u32;
    let offset: u32 = (2u32).wrapping_add(prefix) << nbits;
    let distcode: u32 = (2u32)
        .wrapping_mul(nbits.wrapping_sub(1))
        .wrapping_add(prefix)
        .wrapping_add(80);
    let extra: u32 = d.wrapping_sub(offset);
    (*commands)[0] = distcode | extra << 8;
    let remainder = std::mem::take(commands);
    let _ = std::mem::replace(commands, &mut remainder[1..]);
    1
}
fn Hash(p: &[u8], shift: usize, length: usize) -> u32 {
    let h: u64 = (BROTLI_UNALIGNED_LOAD64(p) << ((8 - length) * 8)).wrapping_mul(K_HASH_MUL32);
    (h >> shift) as u32
}
fn ShouldCompress(input: &[u8], input_size: usize, num_literals: usize) -> bool {
    let corpus_size = input_size as floatX;
    if (num_literals as floatX) < 0.98 * corpus_size {
        true
    } else {
        let mut literal_histo: [u32; 256] = [0; 256];
        let max_total_bit_cost: floatX = corpus_size * 8.0 * 0.98 / 43.0;
        let mut i: usize = 0;
        while i < input_size {
            literal_histo[input[i] as usize] = literal_histo[input[i] as usize].wrapping_add(1);
            i = i.wrapping_add(43);
        }
        BitsEntropy(&literal_histo[..], 256) < max_total_bit_cost
    }
}
fn CreateCommands(
    input_index: usize,
    block_size: usize,
    input_size: usize,
    base_ip: &[u8],
    table: &mut [i32],
    table_bits: usize,
    min_match: usize,
    literals: &mut &mut [u8],
    num_literals: &mut usize,
    commands: &mut &mut [u32],
    num_commands: &mut usize,
) {
    let mut ip_index: usize = input_index;
    let shift: usize = (64u32 as usize).wrapping_sub(table_bits);
    let ip_end: usize = input_index.wrapping_add(block_size);
    let mut next_emit: usize = input_index;
    let mut last_distance: i32 = -1i32;
    let kInputMarginBytes: usize = 16usize;

    if block_size >= kInputMarginBytes {
        let len_limit: usize = min(
            block_size.wrapping_sub(min_match),
            input_size.wrapping_sub(kInputMarginBytes),
        );
        let ip_limit: usize = input_index.wrapping_add(len_limit);
        let mut next_hash: u32;
        let mut goto_emit_remainder = false;
        next_hash = Hash(
            &base_ip[{
                ip_index = ip_index.wrapping_add(1);
                ip_index
            }..],
            shift,
            min_match,
        );
        while !goto_emit_remainder {
            let mut skip: u32 = 32u32;
            let mut next_ip: usize = ip_index;
            let mut candidate: usize = 0;
            loop {
                {
                    'break3: loop {
                        {
                            let hash: u32 = next_hash;
                            let bytes_between_hash_lookups: u32 = skip >> 5;
                            skip = skip.wrapping_add(1);
                            ip_index = next_ip;
                            next_ip = ip_index.wrapping_add(bytes_between_hash_lookups as usize);
                            if next_ip > ip_limit {
                                goto_emit_remainder = true;
                                {
                                    break 'break3;
                                }
                            }
                            next_hash = Hash(&base_ip[next_ip..], shift, min_match);
                            candidate = ip_index.wrapping_sub(last_distance as usize);
                            if IsMatch(&base_ip[ip_index..], &base_ip[candidate..], min_match)
                                && candidate < ip_index
                            {
                                table[(hash as usize)] = ip_index.wrapping_sub(0) as i32;
                                {
                                    break 'break3;
                                }
                            }
                            candidate = table[(hash as usize)] as usize;
                            table[(hash as usize)] = ip_index.wrapping_sub(0) as i32;
                        }
                        if IsMatch(&base_ip[ip_index..], &base_ip[candidate..], min_match) {
                            break;
                        }
                    }
                }
                if !(ip_index.wrapping_sub(candidate)
                    > (1usize << 18).wrapping_sub(16) as isize as usize
                    && !goto_emit_remainder)
                {
                    break;
                }
            }
            if goto_emit_remainder {
                break;
            }
            {
                let base: usize = ip_index;
                let matched: usize = min_match.wrapping_add(FindMatchLengthWithLimit(
                    &base_ip[(candidate + min_match)..],
                    &base_ip[(ip_index + min_match)..],
                    ip_end.wrapping_sub(ip_index).wrapping_sub(min_match),
                ));
                let distance: i32 = base.wrapping_sub(candidate) as i32;
                let insert: i32 = base.wrapping_sub(next_emit) as i32;
                ip_index = ip_index.wrapping_add(matched);
                *num_commands += EmitInsertLen(insert as u32, commands);
                (*literals)[..(insert as usize)]
                    .clone_from_slice(&base_ip[next_emit..(next_emit + insert as usize)]);
                *num_literals += insert as usize;
                let new_literals = std::mem::take(literals);
                let _ = std::mem::replace(literals, &mut new_literals[(insert as usize)..]);
                if distance == last_distance {
                    (*commands)[0] = 64u32;
                    let remainder = std::mem::take(commands);
                    let _ = std::mem::replace(commands, &mut remainder[1..]);
                    *num_commands += 1;
                } else {
                    *num_commands += EmitDistance(distance as u32, commands);
                    last_distance = distance;
                }
                *num_commands += EmitCopyLenLastDistance(matched, commands);
                next_emit = ip_index;
                if ip_index >= ip_limit {
                    goto_emit_remainder = true;
                    {
                        break;
                    }
                }
                {
                    let mut input_bytes: u64;
                    let mut prev_hash: u32;
                    let cur_hash: u32;
                    if min_match == 4 {
                        input_bytes = BROTLI_UNALIGNED_LOAD64(&base_ip[(ip_index - 3)..]);
                        cur_hash = HashBytesAtOffset(input_bytes, 3i32, shift, min_match);
                        prev_hash = HashBytesAtOffset(input_bytes, 0i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(3) as i32;
                        prev_hash = HashBytesAtOffset(input_bytes, 1i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(2) as i32;
                        prev_hash = HashBytesAtOffset(input_bytes, 0i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(1) as i32;
                    } else {
                        assert!(ip_index >= 5);
                        // could this be off the end FIXME
                        input_bytes = BROTLI_UNALIGNED_LOAD64(&base_ip[(ip_index - 5)..]);
                        prev_hash = HashBytesAtOffset(input_bytes, 0i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(5) as i32;
                        prev_hash = HashBytesAtOffset(input_bytes, 1i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(4) as i32;
                        prev_hash = HashBytesAtOffset(input_bytes, 2i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(3) as i32;
                        assert!(ip_index >= 2);
                        input_bytes = BROTLI_UNALIGNED_LOAD64(&base_ip[(ip_index - 2)..]);
                        cur_hash = HashBytesAtOffset(input_bytes, 2i32, shift, min_match);
                        prev_hash = HashBytesAtOffset(input_bytes, 0i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(2) as i32;
                        prev_hash = HashBytesAtOffset(input_bytes, 1i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(1) as i32;
                    }
                    candidate = table[(cur_hash as usize)] as usize;
                    table[(cur_hash as usize)] = ip_index as i32;
                }
            }
            while ip_index.wrapping_sub(candidate)
                <= (1usize << 18).wrapping_sub(16) as isize as usize
                && IsMatch(&base_ip[ip_index..], &base_ip[candidate..], min_match)
            {
                let base_index: usize = ip_index;
                let matched: usize = min_match.wrapping_add(FindMatchLengthWithLimit(
                    &base_ip[(candidate + min_match)..],
                    &base_ip[(ip_index + min_match)..],
                    ip_end.wrapping_sub(ip_index).wrapping_sub(min_match),
                ));
                ip_index = ip_index.wrapping_add(matched);
                last_distance = base_index.wrapping_sub(candidate) as i32;
                *num_commands += EmitCopyLen(matched, commands);
                *num_commands += EmitDistance(last_distance as u32, commands);
                next_emit = ip_index;
                if ip_index >= ip_limit {
                    goto_emit_remainder = true;
                    {
                        break;
                    }
                }
                {
                    assert!(ip_index >= 5);
                    let mut input_bytes: u64;

                    let cur_hash: u32;
                    let mut prev_hash: u32;
                    if min_match == 4 {
                        input_bytes = BROTLI_UNALIGNED_LOAD64(&base_ip[(ip_index - 3)..]);
                        cur_hash = HashBytesAtOffset(input_bytes, 3i32, shift, min_match);
                        prev_hash = HashBytesAtOffset(input_bytes, 0i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(3) as i32;
                        prev_hash = HashBytesAtOffset(input_bytes, 1i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(2) as i32;
                        prev_hash = HashBytesAtOffset(input_bytes, 2i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(1) as i32;
                    } else {
                        input_bytes = BROTLI_UNALIGNED_LOAD64(&base_ip[(ip_index - 5)..]);
                        prev_hash = HashBytesAtOffset(input_bytes, 0i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(5) as i32;
                        prev_hash = HashBytesAtOffset(input_bytes, 1i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(4) as i32;
                        prev_hash = HashBytesAtOffset(input_bytes, 2i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(3) as i32;
                        assert!(ip_index >= 2);
                        input_bytes = BROTLI_UNALIGNED_LOAD64(&base_ip[(ip_index - 2)..]);
                        cur_hash = HashBytesAtOffset(input_bytes, 2i32, shift, min_match);
                        prev_hash = HashBytesAtOffset(input_bytes, 0i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(2) as i32;
                        prev_hash = HashBytesAtOffset(input_bytes, 1i32, shift, min_match);
                        table[(prev_hash as usize)] = ip_index.wrapping_sub(1) as i32;
                    }
                    candidate = table[(cur_hash as usize)] as usize;
                    table[(cur_hash as usize)] = ip_index as i32;
                }
            }
            if !goto_emit_remainder {
                next_hash = Hash(
                    &base_ip[{
                        ip_index = ip_index.wrapping_add(1);
                        ip_index
                    }..],
                    shift,
                    min_match,
                );
            }
        }
    }
    if next_emit < ip_end {
        let insert: u32 = ip_end.wrapping_sub(next_emit) as u32;
        *num_commands += EmitInsertLen(insert, commands);
        literals[..insert as usize]
            .clone_from_slice(&base_ip[next_emit..(next_emit + insert as usize)]);
        let mut xliterals = std::mem::take(literals);
        *literals = &mut std::mem::take(&mut xliterals)[(insert as usize)..];
        *num_literals += insert as usize;
    }
}
fn StoreCommands(
    tree_scratch: &mut [HuffmanTree],

    mut literals: &[u8],
    num_literals: usize,
    commands: &[u32],
    num_commands: usize,
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    static kNumExtraBits: [u32; 128] = [
        0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10, 12, 14, 24, 0, 0, 0, 0, 0,
        0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6,
        7, 8, 9, 10, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5,
        5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 15, 16, 16, 17, 17,
        18, 18, 19, 19, 20, 20, 21, 21, 22, 22, 23, 23, 24, 24,
    ];
    static kInsertOffset: [u32; 24] = [
        0, 1, 2, 3, 4, 5, 6, 8, 10, 14, 18, 26, 34, 50, 66, 98, 130, 194, 322, 578, 1090, 2114,
        6210, 22594,
    ];
    let mut lit_depths: [u8; 256] = [0; 256];
    let mut lit_bits: [u16; 256] = [0; 256]; // maybe return this instead
    let mut lit_histo: [u32; 256] = [0; 256]; // maybe return this instead of init
    let mut cmd_depths: [u8; 128] = [0; 128];
    let mut cmd_bits: [u16; 128] = [0; 128];
    let mut cmd_histo: [u32; 128] = [0; 128];
    let mut i: usize;
    for i in 0usize..num_literals {
        let _rhs = 1;
        let _lhs = &mut lit_histo[literals[i] as usize];
        *_lhs = (*_lhs).wrapping_add(_rhs as u32);
    }
    BrotliBuildAndStoreHuffmanTreeFast(
        &lit_histo[..],
        num_literals,
        8u8,
        &mut lit_depths[..],
        &mut lit_bits[..],
        storage_ix,
        storage,
        &mut tree_scratch[..],
    );
    i = 0usize;
    while i < num_commands {
        {
            let code: u32 = commands[i] & 0xffu32;
            {
                let _rhs = 1;
                let _lhs = &mut cmd_histo[code as usize];
                *_lhs = (*_lhs).wrapping_add(_rhs as u32);
            }
        }
        i = i.wrapping_add(1);
    }
    {
        let _rhs = 1i32;
        let _lhs = &mut cmd_histo[1];
        *_lhs = (*_lhs).wrapping_add(_rhs as u32);
    }
    {
        let _rhs = 1i32;
        let _lhs = &mut cmd_histo[2];
        *_lhs = (*_lhs).wrapping_add(_rhs as u32);
    }
    {
        let _rhs = 1i32;
        let _lhs = &mut cmd_histo[64];
        *_lhs = (*_lhs).wrapping_add(_rhs as u32);
    }
    {
        let _rhs = 1i32;
        let _lhs = &mut cmd_histo[84];
        *_lhs = (*_lhs).wrapping_add(_rhs as u32);
    }
    BuildAndStoreCommandPrefixCode(
        &cmd_histo[..],
        &mut cmd_depths[..],
        &mut cmd_bits[..],
        storage_ix,
        storage,
    );
    for i in 0usize..num_commands {
        let cmd: u32 = commands[i];
        let code: u32 = cmd & 0xffu32;
        let extra: u32 = cmd >> 8;
        BrotliWriteBits(
            cmd_depths[code as usize] as usize,
            u64::from(cmd_bits[code as usize]),
            storage_ix,
            storage,
        );
        BrotliWriteBits(
            kNumExtraBits[code as usize] as usize,
            u64::from(extra),
            storage_ix,
            storage,
        );
        if code < 24u32 {
            let insert: u32 = kInsertOffset[code as usize].wrapping_add(extra);
            for literal in &literals[..(insert as usize)] {
                let lit: u8 = *literal;
                BrotliWriteBits(
                    lit_depths[lit as usize] as usize,
                    u64::from(lit_bits[lit as usize]),
                    storage_ix,
                    storage,
                );
            }
            literals = &literals[insert as usize..];
        }
    }
}
fn BuildAndStoreCommandPrefixCode(
    histogram: &[u32],
    depth: &mut [u8],
    bits: &mut [u16],
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    let mut tree = [HuffmanTree::new(0, 0, 0); 129];
    let mut cmd_depth: [u8; 704] = [0; 704];
    let mut cmd_bits: [u16; 64] = [0; 64];
    BrotliCreateHuffmanTree(histogram, 64usize, 15i32, &mut tree[..], depth);
    BrotliCreateHuffmanTree(
        &histogram[64..],
        64usize,
        14i32,
        &mut tree[..],
        &mut depth[64..],
    );
    /* We have to jump through a few hoops here in order to compute
    the command bits because the symbols are in a different order than in
    the full alphabet. This looks complicated, but having the symbols
    in this order in the command bits saves a few branches in the Emit*
    functions. */
    memcpy(&mut cmd_depth[..], 0, depth, 24, 24);
    memcpy(&mut cmd_depth[..], 24, depth, 0, 8);
    memcpy(&mut cmd_depth[..], 32usize, depth, (48usize), 8usize);
    memcpy(&mut cmd_depth[..], 40usize, depth, (8usize), 8usize);
    memcpy(&mut cmd_depth[..], 48usize, depth, (56usize), 8usize);
    memcpy(&mut cmd_depth[..], 56usize, depth, (16usize), 8usize);
    BrotliConvertBitDepthsToSymbols(&cmd_depth[..], 64usize, &mut cmd_bits[..]);
    memcpy(bits, 0, &cmd_bits[..], 24usize, 16usize);
    memcpy(bits, (8usize), &cmd_bits[..], 40usize, 8usize);
    memcpy(bits, (16usize), &cmd_bits[..], 56usize, 8usize);
    memcpy(bits, (24usize), &cmd_bits[..], 0, 48usize);
    memcpy(bits, (48usize), &cmd_bits[..], 32usize, 8usize);
    memcpy(bits, (56usize), &cmd_bits[..], 48usize, 8usize);
    BrotliConvertBitDepthsToSymbols(&depth[64..], 64usize, &mut bits[64..]);
    {
        for item in &mut cmd_depth[..64] {
            *item = 0;
        }
        //memset(&mut cmd_depth[..], 0i32, 64usize);
        memcpy(&mut cmd_depth[..], 0, depth, (24usize), 8usize);
        memcpy(&mut cmd_depth[..], 64usize, depth, (32usize), 8usize);
        memcpy(&mut cmd_depth[..], 128usize, depth, (40usize), 8usize);
        memcpy(&mut cmd_depth[..], 192usize, depth, (48usize), 8usize);
        memcpy(&mut cmd_depth[..], 384usize, depth, (56usize), 8usize);
        for i in 0usize..8usize {
            cmd_depth[(128usize).wrapping_add((8usize).wrapping_mul(i))] = depth[i];
            cmd_depth[(256usize).wrapping_add((8usize).wrapping_mul(i))] = depth[i.wrapping_add(8)];
            cmd_depth[(448usize).wrapping_add((8usize).wrapping_mul(i))] =
                depth[i.wrapping_add(16)];
        }
        BrotliStoreHuffmanTree(&cmd_depth[..], 704, &mut tree[..], storage_ix, storage);
    }
    BrotliStoreHuffmanTree(&depth[64..], 64, &mut tree[..], storage_ix, storage);
}
fn compress_fragment_two_pass_impl(
    tree_scratch: &mut [HuffmanTree],

    base_ip: &[u8],
    mut input_size: usize,
    is_last: bool,
    command_buf: &mut [u32],
    literal_buf: &mut [u8],
    table: &mut [i32],
    table_bits: usize,
    min_match: usize,
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    let mut input_index: usize = 0usize;
    while input_size > 0usize {
        let block_size: usize = min(input_size, kCompressFragmentTwoPassBlockSize);
        let mut num_literals: usize = 0;
        let mut num_commands: usize = 0;
        {
            let mut literals = &mut literal_buf[..];
            let mut commands = &mut command_buf[..];
            CreateCommands(
                input_index,
                block_size,
                input_size,
                base_ip,
                table,
                table_bits,
                min_match,
                &mut literals,
                &mut num_literals,
                &mut commands,
                &mut num_commands,
            );
        }
        if ShouldCompress(&base_ip[input_index..], block_size, num_literals) {
            store_meta_block_header(block_size, false, storage_ix, storage);
            BrotliWriteBits(13usize, 0, storage_ix, storage);
            StoreCommands(
                &mut tree_scratch[..],
                literal_buf,
                num_literals,
                command_buf,
                num_commands,
                storage_ix,
                storage,
            );
        } else {
            EmitUncompressedMetaBlock(&base_ip[input_index..], block_size, storage_ix, storage);
        }
        input_index = input_index.wrapping_add(block_size);
        input_size = input_size.wrapping_sub(block_size);
    }
}
fn EmitUncompressedMetaBlock(
    input: &[u8],
    input_size: usize,
    storage_ix: &mut usize,
    storage: &mut [u8],
) {
    store_meta_block_header(input_size, true, storage_ix, storage);
    *storage_ix = storage_ix.wrapping_add(7u32 as usize) & !7u32 as usize;
    memcpy(storage, (*storage_ix >> 3), input, 0, input_size);
    *storage_ix = storage_ix.wrapping_add(input_size << 3);
    storage[(*storage_ix >> 3)] = 0u8;
}
fn RewindBitPosition(new_storage_ix: usize, storage_ix: &mut usize, storage: &mut [u8]) {
    let bitpos: usize = new_storage_ix & 7usize;
    let mask: usize = (1u32 << bitpos).wrapping_sub(1) as usize;
    {
        let _rhs = mask as u8;
        let _lhs = &mut storage[(new_storage_ix >> 3)];
        *_lhs = (i32::from(*_lhs) & i32::from(_rhs)) as u8;
    }
    *storage_ix = new_storage_ix;
}

// ── Public entry point ──

/// The reference q1 path: two-pass fragment compression with reference
/// table sizing (GetHashTable(q1): 256 doubling to min(1<<17, n)).
/// Used by `compress_with_quality` for q0-1.
#[must_use]
pub fn compress_two_pass_q1(input: &[u8]) -> Vec<u8> {
    vendored_compress(input)
}

#[must_use]
pub fn vendored_compress(input: &[u8]) -> Vec<u8> {
    let n = input.len();
    let mut storage: Vec<u8> = vec![0u8; n * 2 + 4096];
    let mut storage_ix: usize = 0;

    // Frame header: lgwin=22 (4MB window, brotli default).
    // Per upstream EncodeWindowBits: lgwin > 17 → write 4 bits value ((lgwin-17)<<1)|1.
    // For lgwin=22: value = (5<<1)|1 = 0b1011 = 11.
    BrotliWriteBits(4, 0b1011, &mut storage_ix, &mut storage);

    if n == 0 {
        BrotliWriteBits(1, 1, &mut storage_ix, &mut storage);
        BrotliWriteBits(1, 1, &mut storage_ix, &mut storage);
        storage_ix = (storage_ix + 7) & !7;
        let out_len = storage_ix.div_ceil(8);
        return storage[..out_len].to_vec();
    }

    // Reference GetHashTable(q1): double from 256 while below both
    // 1<<17 and the input length; min_match 6 once table_bits >= 15.
    let mut htsize: usize = 256;
    while htsize < (1 << 17) && htsize < n {
        htsize <<= 1;
    }
    let table_bits: usize = htsize.trailing_zeros() as usize;
    let table_size = htsize;
    let mut table: Vec<i32> = vec![0; table_size];
    let mut command_buf: Vec<u32> = vec![0u32; n + 1];
    let mut literal_buf: Vec<u8> = vec![0u8; n];
    let mut tree_scratch: Vec<HuffmanTree> = vec![HuffmanTree::default(); 2 * 704 + 1];

    let min_match = if table_bits < 15 { 4 } else { 6 };
    let initial_storage_ix = storage_ix;
    compress_fragment_two_pass_impl(
        &mut tree_scratch[..],
        input,
        n,
        true,
        &mut command_buf,
        &mut literal_buf,
        &mut table,
        table_bits,
        min_match,
        &mut storage_ix,
        &mut storage,
    );
    // If compressed output is larger than uncompressed + 31, emit uncompressed instead.
    if storage_ix.wrapping_sub(initial_storage_ix) > 31usize.wrapping_add(n << 3) {
        RewindBitPosition(initial_storage_ix, &mut storage_ix, &mut storage);
        EmitUncompressedMetaBlock(input, n, &mut storage_ix, &mut storage);
    }

    // Final empty ISLAST+ISEMPTY metablock (marks end of stream).
    BrotliWriteBits(1, 1, &mut storage_ix, &mut storage);
    BrotliWriteBits(1, 1, &mut storage_ix, &mut storage);
    storage_ix = (storage_ix + 7) & !7;

    let out_len = storage_ix.div_ceil(8);
    storage.truncate(out_len);
    storage
}

// --- Missing helpers ---

#[allow(non_camel_case_types)]
type floatX = f32;

fn FastLog2u16(v: u16) -> floatX {
    if v == 0 {
        0.0
    } else {
        f32::from(v).log2()
    }
}
fn FastLog2(v: u64) -> floatX {
    if v == 0 {
        0.0
    } else {
        (v as f32).log2()
    }
}

fn shannon_entropy(population: &[u32], size: usize) -> (floatX, usize) {
    let mut sum: usize = 0;
    let mut retval: floatX = 0.0;
    let mut start = 0;
    if (size & 1) != 0 && !population.is_empty() {
        let p = population[0] as usize;
        sum = sum.wrapping_add(p);
        retval -= p as floatX * FastLog2u16(p as u16);
        start = 1;
    }
    let even_size = (size >> 1) << 1;
    for i in start..even_size {
        let p = population[i] as usize;
        sum = sum.wrapping_add(p);
        retval -= p as floatX * FastLog2u16(p as u16);
    }
    if sum != 0 {
        retval += sum as floatX * FastLog2(sum as u64);
    }
    (retval, sum)
}

fn BitsEntropy(population: &[u32], size: usize) -> floatX {
    let (mut retval, sum) = shannon_entropy(population, size);
    if retval < sum as floatX {
        retval = sum as floatX;
    }
    retval
}

fn HashBytesAtOffset(v: u64, offset: i32, shift: usize, length: usize) -> u32 {
    let h: u64 = (v >> (8i32 * offset) << ((8 - length) * 8)).wrapping_mul(K_HASH_MUL32);
    (h >> shift) as u32
}

fn IsMatch(p1: &[u8], p2: &[u8], min_match: usize) -> bool {
    FindMatchLengthWithLimit(p1, p2, min_match) >= min_match
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_huffman_tree_simple() {
        let mut histo = [0u32; 8];
        histo[0] = 10;
        histo[1] = 5;
        histo[2] = 3;
        let mut depth = [0u8; 8];
        let mut tree = vec![HuffmanTree::default(); 17];
        let ok = BrotliCreateHuffmanTree(&histo, 8, 15, &mut tree, &mut depth);
        assert!(ok);
        // Higher frequency symbols should get shorter codes
        assert!(
            depth[0] <= depth[2],
            "sym0 (freq 10) should be <= sym2 (freq 3): {} vs {}",
            depth[0],
            depth[2]
        );
    }

    #[test]
    fn vendored_compress_round_trips_via_cli() {
        let inputs: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"a".to_vec(),
            b"abcd".to_vec(),
            b"abcdabcd".to_vec(),
            b"hello world".to_vec(),
            b"aaaaaaaaaa".to_vec(),
            b"a".repeat(100),
            b"hello world ".repeat(10),
            b"The quick brown fox. ".repeat(20),
            (0u8..=255).cycle().take(500).collect(),
            b"a".repeat(1000),
        ];

        let tmp = std::env::temp_dir().join("omnizip_brotli_cli_test.br");
        for (i, input) in inputs.iter().enumerate() {
            let encoded = vendored_compress(input);
            std::fs::write(&tmp, &encoded).expect("write");
            let result = std::process::Command::new("brotli")
                .arg("-d")
                .arg("-c")
                .arg(&tmp)
                .output();
            match result {
                Ok(output) if output.status.success() => {
                    assert_eq!(
                        output.stdout,
                        *input,
                        "brotli -d mismatch for input #{} ({} bytes): got {} bytes",
                        i,
                        input.len(),
                        output.stdout.len()
                    );
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    panic!(
                        "brotli -d failed for input #{} ({} bytes): {stderr}",
                        i,
                        input.len()
                    );
                }
                Err(e) => {
                    eprintln!("[skip] brotli CLI not installed: {e}");
                    return;
                }
            }
        }
    }
}
