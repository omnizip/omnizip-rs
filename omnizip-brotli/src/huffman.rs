//! Brotli Huffman tree construction and emission (RFC 7932 §9.5).
//!
//! Ported from `brotli/src/enc/brotli_bit_stream.rs`. Builds a
//! canonical Huffman tree from a histogram and emits it in either
//! simple form (≤ 4 symbols) or complex form (with RLE).

#![forbid(unsafe_code)]

use crate::encoder::BitWriter;

/// Maximum code length per RFC 7932 §9.5.
pub const MAX_HUFFMAN_CODE_LENGTH: u8 = 15;

/// A Huffman tree node used during construction.
#[derive(Clone, Copy, Default)]
struct HuffmanTree {
    #[allow(dead_code)]
    total_count: u32,
    index_left: i32,
    index_right_or_value: i32,
}

impl HuffmanTree {
    const fn new(count: u32, left: i32, right: i32) -> Self {
        Self {
            total_count: count,
            index_left: left,
            index_right_or_value: right,
        }
    }
}

/// Sort tree items by `total_count` (ascending). Mirrors upstream
/// `SortHuffmanTreeItems` with `SimpleSortHuffmanTree`.
#[cfg(test)]
fn sort_huffman_tree_items(tree: &mut [HuffmanTree], n: usize) {
    tree[..n].sort_by_key(|t| t.total_count);
}

/// Recursively set depth on each leaf. Returns false if max_depth
/// would be exceeded. Ported from `BrotliSetDepth`.
fn set_depth(p0: i32, pool: &mut [HuffmanTree], depth: &mut [u8], max_depth: i32) -> bool {
    let mut stack: Vec<(i32, i32)> = Vec::with_capacity(16);
    stack.push((p0, 0));
    while let Some((node_idx, level)) = stack.pop() {
        let node = pool[node_idx as usize];
        if node.index_left >= 0 {
            // Internal node — recurse into children.
            stack.push((node.index_left, level + 1));
            stack.push((node.index_right_or_value, level + 1));
        } else if node.index_right_or_value >= 0 {
            // Leaf — record the depth.
            let sym = node.index_right_or_value as usize;
            if sym >= depth.len() {
                return false;
            }
            depth[sym] = level as u8;
            if level > max_depth {
                return false;
            }
        }
    }
    true
}

/// Reverse the low `num_bits` bits of `bits`. Used by
/// `BrotliConvertBitDepthsToSymbols` to produce MSB-first Huffman
/// codes for emission into the LSB-first bitstream. Mirrors upstream
/// `BrotliReverseBits` from entropy_encode.rs.
fn reverse_bits(num_bits: usize, mut bits: u16) -> u16 {
    const LUT: [u16; 16] = [
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
    retval >> (4 - (num_bits & 3)) * 1
}

/// Build canonical Huffman codes from per-symbol code lengths.
///
/// Mirrors `BrotliConvertBitDepthsToSymbols`. **Codes are bit-reversed**
/// before being stored, because brotli's bitstream is LSB-first within
/// each byte but Huffman codes are conventionally MSB-first.
#[must_use]
pub fn convert_bit_depths_to_symbols(depth: &[u8]) -> Vec<u16> {
    const MAX_HUFFMAN_BITS: usize = 16;
    let n = depth.len();
    let mut codes = vec![0u16; n];
    let mut bl_count = [0u16; MAX_HUFFMAN_BITS];
    for &d in depth {
        if d > 0 {
            bl_count[d as usize] += 1;
        }
    }
    bl_count[0] = 0;
    let mut next_code = [0u16; MAX_HUFFMAN_BITS];
    let mut code: i32 = 0;
    for i in 1..MAX_HUFFMAN_BITS {
        code = (code + bl_count[i - 1] as i32) << 1;
        next_code[i] = code as u16;
    }
    for i in 0..n {
        if depth[i] != 0 {
            let d = depth[i] as usize;
            codes[i] = reverse_bits(d, next_code[d]);
            next_code[d] += 1;
        }
    }
    codes
}

/// Build the per-symbol code lengths from a histogram. Returns
/// `(depth, bits)` arrays.
///
/// Uses the standard min-heap Huffman construction. Code lengths are
/// capped at 15 bits via the iterative "kraft inequality" repair
/// algorithm (matches upstream's count_limit elevation when needed).
#[must_use]
pub fn build_huffman_tree(histogram: &[u32], alphabet_size: usize) -> (Vec<u8>, Vec<u16>) {
    let mut depth = vec![0u8; alphabet_size];

    // Collect symbols with non-zero frequency.
    let active: Vec<(u32, usize)> = histogram
        .iter()
        .enumerate()
        .filter(|(_, &f)| f > 0)
        .map(|(i, &f)| (f, i))
        .collect();

    if active.is_empty() {
        let bits = convert_bit_depths_to_symbols(&depth);
        return (depth, bits);
    }
    if active.len() == 1 {
        // Single-symbol case: assign 1-bit code. The encoder typically
        // adds a dummy second symbol (handled at the call site) to
        // avoid the encoder/decoder mismatch in NSYM=1 simple-form.
        depth[active[0].1] = 1;
        let bits = convert_bit_depths_to_symbols(&depth);
        return (depth, bits);
    }

    // Iterative Huffman with count-limit elevation (matches upstream).
    let mut count_limit = 1u32;
    loop {
        // Build a min-heap with elevated counts.
        let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(u32, i32)>> =
            std::collections::BinaryHeap::new();
        let mut nodes: Vec<HuffmanTree> = Vec::new();
        for &(f, sym) in &active {
            let elevated = f.max(count_limit);
            let idx = nodes.len() as i32;
            nodes.push(HuffmanTree::new(elevated, -1, sym as i32));
            heap.push(std::cmp::Reverse((elevated, idx)));
        }

        // Push a sentinel so the loop terminates cleanly.
        let sentinel_idx = nodes.len() as i32;
        nodes.push(HuffmanTree::new(u32::MAX, -1, -1));
        heap.push(std::cmp::Reverse((u32::MAX, sentinel_idx)));

        // Merge until one root remains. We keep the sentinel in the
        // heap to terminate cleanly: when only root + sentinel remain,
        // we pop the root (which has a smaller count than u32::MAX).
        let mut root_idx: Option<i32> = None;
        let mut root_count: Option<u32> = None;
        while let Some(std::cmp::Reverse((c, idx))) = heap.pop() {
            if c == u32::MAX {
                // Sentinel — stop.
                break;
            }
            if let (Some(rc), Some(ri)) = (root_count, root_idx) {
                // Already have a root; merge it with the new one.
                let merged_count = rc.saturating_add(c);
                let merged_idx = nodes.len() as i32;
                nodes.push(HuffmanTree::new(merged_count, ri, idx));
                root_count = Some(merged_count);
                root_idx = Some(merged_idx);
            } else {
                root_count = Some(c);
                root_idx = Some(idx);
            }
        }
        let root_idx = root_idx.expect("root exists");

        // Set depth on each leaf.
        for d in depth.iter_mut() {
            *d = 0;
        }
        if set_depth(root_idx, &mut nodes, &mut depth, 15) {
            break;
        }
        count_limit = count_limit.saturating_mul(2);
        if count_limit > (1 << 30) {
            break;
        }
    }

    let bits = convert_bit_depths_to_symbols(&depth);
    (depth, bits)
}

/// Emit a Huffman tree in simple form (≤ 4 symbols). Mirrors
/// upstream's `BrotliBuildAndStoreHuffmanTreeFast` simple-form path.
///
/// Returns `true` if emitted, `false` if too many symbols (use
/// complex form instead).
pub(crate) fn store_simple_form(
    symbols: &[u64],
    depth: &[u8],
    max_bits: u8,
    bw: &mut BitWriter,
) -> bool {
    let count = symbols.len();
    if count > 4 {
        return false;
    }
    if count == 1 {
        // HSKIP=1, NSYM=0 → 4 bits = 0b0001.
        // The "1" in the 2-bit HSKIP position is the decoder's marker
        // for "simple form". Setting HSKIP=1 here matches upstream
        // `BrotliBuildAndStoreHuffmanTreeFast` exactly.
        bw.write_bits(1, 4);
        bw.write_bits(symbols[0], u32::from(max_bits));
        return true;
    }
    // HSKIP=1 (the "simple form" marker) + NSYM_raw=count-1.
    // Upstream: `BrotliWriteBits(2, 1, ...)` for HSKIP, then
    // `BrotliWriteBits(2, count-1, ...)` for NSYM-1.
    bw.write_bits(1, 2);
    bw.write_bits((count - 1) as u64, 2);

    // Sort symbols by descending depth (upstream convention).
    let mut sorted: Vec<u64> = symbols.to_vec();
    for i in 0..count {
        for j in i + 1..count {
            if depth[symbols[j] as usize] < depth[symbols[i] as usize] {
                sorted.swap(i, j);
            }
        }
    }

    for &s in &sorted {
        bw.write_bits(s, u32::from(max_bits));
    }

    // Per upstream BrotliBuildAndStoreHuffmanTreeFast:
    // - count==2: NO tree_select (both symbols get 1-bit codes)
    // - count==3: NO tree_select (all get 2-bit codes)
    // - count==4: 1-bit tree_select
    if count == 4 {
        let tree_select = if depth[sorted[0] as usize] == 1 { 1 } else { 0 };
        bw.write_bits(u64::from(tree_select as u32), 1);
    }
    true
}

/// Build and emit a Huffman tree in simple form if possible,
/// otherwise return false (caller should fall back to complex form).
///
/// Returns `(emitted, symbols)` where `emitted` is true if simple
/// form was used.
pub(crate) fn build_and_store_simple(
    histogram: &[u32],
    alphabet_size: usize,
    max_bits: u8,
    bw: &mut BitWriter,
) -> (bool, Vec<u8>, Vec<u16>) {
    let (depth, bits) = build_huffman_tree(histogram, alphabet_size);

    // Collect symbols in alphabetical order.
    let symbols: Vec<u64> = (0..alphabet_size as u64)
        .filter(|&i| histogram[i as usize] > 0)
        .collect();

    if symbols.len() <= 4 {
        let emitted = store_simple_form(&symbols, &depth, max_bits, bw);
        (emitted, depth, bits)
    } else {
        (false, depth, bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_symbol_alphabet_gets_one_bit_code() {
        // Single-symbol case: 1-bit code. The encoder's call site
        // adds a dummy second symbol to keep the decoder's simple-form
        // path well-defined (see encode_huffman).
        let mut histo = vec![0u32; 4];
        histo[2] = 10;
        let (depth, _bits) = build_huffman_tree(&histo, 4);
        assert_eq!(depth[2], 1);
    }

    #[test]
    fn two_symbol_alphabet_gets_1_bit_codes() {
        let mut histo = vec![0u32; 4];
        histo[0] = 5;
        histo[1] = 5;
        let (depth, _bits) = build_huffman_tree(&histo, 4);
        assert_eq!(depth[0], 1);
        assert_eq!(depth[1], 1);
    }

    #[test]
    fn skewed_distribution_shorter_for_high_freq() {
        let mut histo = vec![0u32; 4];
        histo[0] = 100;
        histo[1] = 1;
        histo[2] = 1;
        histo[3] = 1;
        let (depth, _bits) = build_huffman_tree(&histo, 4);
        // Symbol 0 (high freq) should have the shortest code.
        let d0 = depth[0];
        let d1 = depth[1];
        assert!(d0 <= d1, "high-freq symbol {d0} should be ≤ low-freq {d1}");
    }

    #[test]
    fn convert_bit_depths_basic() {
        // Canonical assignment + bit reversal (brotli emits Huffman
        // codes MSB-first into the LSB-first bitstream).
        // depth [2, 1, 3, 3]:
        //   len=1: sym 1 → next_code=0 → reverse_bits(1, 0) = 0
        //   len=2: sym 0 → next_code=0+0=0... wait, next_code[2] = 0
        //          Hmm actually next_code[2] is set by (0+0)<<1 = 0.
        //          next_code[3] = (0+1)<<1 = 2.
        //   So sym 0 (len 2): code 0 → reverse_bits(2, 0) = 0b00 reversed = 0b00 = 0
        //   Wait, code 0 for len 2 = "00" MSB-first. Reversed for 2 bits: "00" = 0.
        //   Hmm let me just compute via the algorithm.
        let depth = vec![2, 1, 3, 3];
        let bits = convert_bit_depths_to_symbols(&depth);
        // Canonical codes (before reversal):
        //   sym 1 (len 1): code 0
        //   sym 0 (len 2): code 10 = 2
        //   sym 2 (len 3): code 110 = 6
        //   sym 3 (len 3): code 111 = 7
        // After bit-reversal for emission:
        //   sym 1: 0 → 0 (1 bit reversed)
        //   sym 0: 10 → 01 = 1 (2 bits reversed)
        //   sym 2: 110 → 011 = 3 (3 bits reversed)
        //   sym 3: 111 → 111 = 7 (3 bits reversed)
        assert_eq!(bits[1], 0); // reversed 0
        assert_eq!(bits[0], 1); // reversed 10
        assert_eq!(bits[2], 3); // reversed 110
        assert_eq!(bits[3], 7); // reversed 111
    }

    #[test]
    fn simple_form_emits_for_one_symbol() {
        let mut bw = BitWriter::new();
        let symbols = vec![42u64];
        let depth = vec![0u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                         0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        let emitted = store_simple_form(&symbols, &depth, 8, &mut bw);
        assert!(emitted);
        // 4 bits header + 8 bits symbol = 12 bits.
        assert_eq!(bw.bit_pos_after(), 12);
    }

    #[test]
    fn simple_form_emits_for_two_symbols() {
        let mut bw = BitWriter::new();
        let symbols = vec![10u64, 20];
        let depth = vec![0u8; 256];
        let emitted = store_simple_form(&symbols, &depth, 8, &mut bw);
        assert!(emitted);
        // HSKIP(2) + NSYM-1(2) + symbol1(8) + symbol2(8) = 20 bits.
        // No tree_select for count==2.
        assert_eq!(bw.bit_pos_after(), 20);
    }
}