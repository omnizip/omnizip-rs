//! Full RFC 7932 brotli decoder path — handles non-trivial metablocks.
//!
//! The trivial-layout fast path lives in `decoder.rs`. This module
//! implements the general-case decoder that handles:
//!
//! - Multi-block-type metablocks (`NBLTYPES > 1` per category).
//! - Literal + distance context maps (RFC 7932 §9.6).
//! - Multi-tree Huffman groups indexed by context.
//! - All four context modes (LSB6, MSB6, UTF8, SIGNED) via
//!   `ContextMode::context_id_2`.
//! - Static dictionary references with transforms (§10.3) when
//!   `distance_code > max_distance`.
//!
//! ## Architecture (OCP)
//!
//! `decode()` in `decoder.rs` dispatches between:
//! - `decode_compressed_metablock` — trivial fast path (no block
//!   switches, single Huffman tree per category, NTREES=1).
//! - `decode_compressed_metablock_full` (this module) — general path.
//!
//! Both share `BitReader`, `HuffmanTable`, `read_huffman_table`,
//! `take_distance_from_ring_buffer`, `decode_distance_from_code`.

#![forbid(unsafe_code)]

use crate::decoder::{
    read_huffman_table, read_varlen_uint8, BitReader, ContextMode, HuffmanTable,
};
use crate::prefix::kBlockLengthPrefixCode;

/// Number of distance context bits (RFC 7932 §10.4).
const K_DISTANCE_CONTEXT_BITS: u32 = 6;

/// Number of literal context bits (RFC 7932 §10.1).
const K_LITERAL_CONTEXT_BITS: u32 = 6;

/// Per-category block-type state (RFC 7932 §9.3).
///
/// One instance per category (literal / insert-copy / distance).
/// Tracks the current block type, the last two block types for the
/// ring-buffer decode of switch codes, and the remaining length in
/// the current block.
#[derive(Clone, Debug)]
pub(crate) struct BlockTypeState {
    /// Number of block types in this category (1..=256).
    pub num_block_types: u32,
    /// Ring buffer of last two block types: `[most_recent, second_recent]`.
    /// Initialised to `[1, 0]` per upstream convention.
    pub block_type_rb: [u32; 2],
    /// Remaining number of bytes in the current block before a
    /// block-switch command is required.
    pub block_length: u32,
    /// Huffman tree for block-type switch codes (alphabet 2 + NBLTYPES).
    /// `None` when `num_block_types == 1` (no switches emitted).
    pub block_type_tree: Option<HuffmanTable>,
    /// Huffman tree for block-length codes (alphabet 26).
    /// `None` when `num_block_types == 1`.
    pub block_len_tree: Option<HuffmanTable>,
}

impl Default for BlockTypeState {
    fn default() -> Self {
        Self {
            num_block_types: 1,
            block_type_rb: [1, 0],
            block_length: 0,
            block_type_tree: None,
            block_len_tree: None,
        }
    }
}

impl BlockTypeState {
    /// Read the per-category block-type code trees and initial state
    /// (RFC 7932 §9.3). Caller has already consumed the NBLTYPES
    /// `DecodeVarLenUint8` field; `bit_pos` points at the block-type
    /// code Huffman tree (or, for `num_block_types == 1`, the next
    /// category's data).
    ///
    /// Layout when `num_block_types == 1`: nothing further.
    /// Layout when `num_block_types > 1`:
    /// 1. Block-type code Huffman tree (alphabet `2 + NBLTYPES`).
    /// 2. Block-length code Huffman tree (alphabet 26).
    /// 3. Initial block type (Huffman symbol from block-type tree).
    /// 4. Initial block length (Huffman symbol + extra bits via
    ///    `kBlockLengthPrefixCode`).
    pub(crate) fn read_block_type_trees(
        &mut self,
        data: &[u8],
        bit_pos: usize,
    ) -> Result<usize, &'static str> {
        let mut br = BitReader::new(data);
        br.bit_pos = bit_pos;

        self.block_type_rb = [1, 0];
        if self.num_block_types == 1 {
            return Ok(br.bit_pos());
        }

        // Block-type code tree: alphabet size 2 + NBLTYPES.
        let alphabet_size = 2 + self.num_block_types;
        let (tree, p) = read_huffman_table(data, br.bit_pos(), alphabet_size as usize)?;
        self.block_type_tree = Some(tree);
        br.bit_pos = p;

        // Block-length code tree: alphabet size 26 (kBlockLengthPrefixCode).
        let (tree, p) = read_huffman_table(data, br.bit_pos(), 26)?;
        self.block_len_tree = Some(tree);
        br.bit_pos = p;

        // Initial block type via the block-type tree.
        let bt_tree = self.block_type_tree.as_ref().unwrap();
        let mut block_type = bt_tree.read_symbol(&mut br).ok_or("invalid block-type symbol")?;
        // Decode ring-buffer convention (matches upstream DecodeBlockTypeAndLength).
        block_type = match block_type {
            0 => self.block_type_rb[0],
            1 => self.block_type_rb[1] + 1,
            other => other - 2,
        };
        if block_type >= self.num_block_types {
            block_type -= self.num_block_types;
        }
        self.block_type_rb[0] = self.block_type_rb[1];
        self.block_type_rb[1] = block_type;

        // Initial block length via the block-length tree.
        let bl_tree = self.block_len_tree.as_ref().unwrap();
        self.block_length = read_block_length(bl_tree, &mut br)?;

        Ok(br.bit_pos())
    }

    /// Decode a block-switch command mid-metablock (RFC 7932 §9.3).
    ///
    /// Updates `block_type_rb` and `block_length`. Returns the new
    /// active block type so callers can refresh context state.
    pub(crate) fn decode_switch(&mut self, br: &mut BitReader) -> Result<u32, &'static str> {
        let bt_tree = self
            .block_type_tree
            .as_ref()
            .ok_or("block switch with num_block_types == 1")?;
        let bl_tree = self
            .block_len_tree
            .as_ref()
            .ok_or("block switch with num_block_types == 1")?;

        let mut block_type = bt_tree.read_symbol(br).ok_or("invalid block-type symbol")?;
        block_type = match block_type {
            0 => self.block_type_rb[0],
            1 => self.block_type_rb[1] + 1,
            other => other - 2,
        };
        if block_type >= self.num_block_types {
            block_type -= self.num_block_types;
        }
        self.block_type_rb[0] = self.block_type_rb[1];
        self.block_type_rb[1] = block_type;
        self.block_length = read_block_length(bl_tree, br)?;
        Ok(block_type)
    }
}

/// Decode a block-length value (RFC 7932 §9.3 + §10.5).
///
/// Reads one Huffman symbol giving the block-length prefix code, then
/// reads `nbits` extra bits. Returns `offset + extra`.
fn read_block_length(tree: &HuffmanTable, br: &mut BitReader) -> Result<u32, &'static str> {
    let code = tree.read_symbol(br).ok_or("invalid block-length symbol")? as usize;
    if code >= kBlockLengthPrefixCode.len() {
        return Err("block-length code out of range");
    }
    let entry = &kBlockLengthPrefixCode[code];
    let extra = br.read_bits(entry.nbits as u32);
    Ok(entry.offset as u32 + extra)
}

/// Read a context map (RFC 7932 §9.6).
///
/// Returns the context map as a `Vec<u8>` and the number of distinct
/// Huffman-tree indices it contains (i.e. `NTREES`).
///
/// Layout:
/// 1. `num_htrees` via `DecodeVarLenUint8 + 1`.
/// 2. If `num_htrees <= 1`: zero-filled context map, no further data.
/// 3. Else: optional RLE flag (1 bit), then context-map code Huffman
///    tree (alphabet `max_rle + num_htrees`), then the per-context
///    entries with optional run-length encoding of zeros, then an
///    optional inverse-MTF flag (1 bit).
pub(crate) fn read_context_map(
    data: &[u8],
    bit_pos: usize,
    context_map_size: usize,
    max_rle_override: u32,
) -> Result<(Vec<u8>, u32, usize), &'static str> {
    let mut br = BitReader::new(data);
    br.bit_pos = bit_pos;

    let num_htrees = read_varlen_uint8(&mut br)? + 1;
    let mut context_map = vec![0u8; context_map_size];

    if num_htrees <= 1 {
        // Trivial: every context maps to tree 0.
        return Ok((context_map, num_htrees, br.bit_pos()));
    }

    // RLE flag.
    let use_rle;
    let max_run_length_prefix;
    if br.read_bits(1) != 0 {
        use_rle = true;
        max_run_length_prefix = (br.read_bits(4) >> 1) + 1;
    } else {
        use_rle = false;
        max_run_length_prefix = 0;
    }
    let _ = use_rle;
    let max_rle = max_run_length_prefix.max(max_rle_override);

    // Context-map code Huffman tree.
    let alphabet_size = max_rle + num_htrees;
    let (cm_tree, p) = read_huffman_table(data, br.bit_pos(), alphabet_size as usize)?;
    br.bit_pos = p;

    // Read context-map entries.
    let mut i = 0usize;
    while i < context_map_size {
        let code = cm_tree.read_symbol(&mut br).ok_or("invalid context-map symbol")?;
        if code == 0 {
            context_map[i] = 0;
            i += 1;
        } else if code > max_run_length_prefix {
            context_map[i] = (code - max_run_length_prefix) as u8;
            i += 1;
        } else {
            // RLE: code bits encode a run length; replicate 0 for that many entries.
            let reps_extra = br.read_bits(code);
            let reps = (1u32 << code) + reps_extra;
            if i + reps as usize > context_map_size {
                return Err("context-map RLE overflows map size");
            }
            for _ in 0..reps {
                context_map[i] = 0;
                i += 1;
            }
        }
    }

    // Optional inverse-MTF transform.
    if br.read_bits(1) != 0 {
        inverse_move_to_front(&mut context_map);
    }

    Ok((context_map, num_htrees, br.bit_pos()))
}

/// Inverse Move-to-Front transform (RFC 7932 §9.6).
///
/// Uses the standard sliding algorithm with a stack-of-8 optimisation
/// since most context-map values are < 8.
fn inverse_move_to_front(v: &mut [u8]) {
    let mut mtf: [u8; 256] = [0; 256];
    for (i, slot) in mtf.iter_mut().enumerate() {
        *slot = i as u8;
    }
    for byte in v.iter_mut() {
        let idx = *byte as usize;
        let value = mtf[idx];
        if idx != 0 {
            // Slide everything above idx down one step, then put value at front.
            mtf.copy_within(..idx, 1);
            mtf[0] = value;
        }
        *byte = value;
    }
}

/// Read a Huffman tree group (RFC 7932 §9.4 `HUFFMAN_TREE_GROUP`).
///
/// Reads `num_trees` Huffman tables of the same alphabet size into a
/// flat `Vec<HuffmanTable>`. Indexed by context map lookup.
pub(crate) fn read_tree_group(
    data: &[u8],
    bit_pos: usize,
    alphabet_size: usize,
    num_trees: u32,
) -> Result<(Vec<HuffmanTable>, usize), &'static str> {
    let mut trees = Vec::with_capacity(num_trees as usize);
    let mut p = bit_pos;
    for _ in 0..num_trees {
        let (tree, np) = read_huffman_table(data, p, alphabet_size)?;
        trees.push(tree);
        p = np;
    }
    Ok((trees, p))
}

// ---------------------------------------------------------------------------
// Static dictionary (RFC 7932 §10.3 + Appendix B) — placeholders for the
// large constant tables that need porting. These are private and tagged
// `#[allow(dead_code)]` until the command loop wires them in.
// ---------------------------------------------------------------------------

/// Look up a dictionary reference and copy the transformed bytes into
/// `output` (RFC 7932 §10.3).
///
/// Returns `Some(len)` if the reference is valid, `None` to fall through
/// to the regular distance handling. This stub returns `None` until the
/// static dictionary data tables are ported (TODO 172 step 4).
fn dictionary_lookup(
    _output: &mut Vec<u8>,
    _copy_len: u32,
    _distance_code: i32,
    _max_distance: u32,
) -> Option<usize> {
    None
}

/// Decode a Huffman-coded metablock using the full RFC 7932 grammar
/// (block-type machinery, context maps, multi-tree groups, context
/// modes, static dictionary).
///
/// Caller is `decode()` in `decoder.rs`, which dispatches here when
/// any category has `num_block_types > 1` or `num_huffman_trees > 1`.
/// `nbltypesl/c/d` are the values already read by the caller (caller
/// advances the bit reader past them, so `bit_pos` points to the
/// NPOSTFIX field here).
///
/// Returns `(new_bit_pos, output_bytes)`.
#[allow(clippy::too_many_lines)]
pub(crate) fn decode_compressed_metablock_full(
    data: &[u8],
    bit_pos: usize,
    mlen: usize,
    nbltypesl: u32,
    nbltypesc: u32,
    nbltypesd: u32,
) -> Result<(usize, Vec<u8>), &'static str> {
    let mut br = BitReader::new(data);
    br.bit_pos = bit_pos;

    // Per-category block-type code reading. BlockTypeState::read_with_count
    // skips the NBLTYPES read (caller has already consumed it).
    let mut lit_bt = BlockTypeState::default();
    lit_bt.num_block_types = nbltypesl;
    br.bit_pos = lit_bt.read_block_type_trees(data, br.bit_pos())?;
    let mut cmd_bt = BlockTypeState::default();
    cmd_bt.num_block_types = nbltypesc;
    br.bit_pos = cmd_bt.read_block_type_trees(data, br.bit_pos())?;
    let mut dist_bt = BlockTypeState::default();
    dist_bt.num_block_types = nbltypesd;
    br.bit_pos = dist_bt.read_block_type_trees(data, br.bit_pos())?;

    // ----- NPOSTFIX + NDIRECT -----
    let npostfix = br.read_bits(2) as usize;
    let ndirect_raw = br.read_bits(4) as usize;
    let ndirect = if ndirect_raw < 12 { ndirect_raw } else { (ndirect_raw - 12) << npostfix };
    if npostfix > 3 {
        return Err("invalid metablock: NPOSTFIX > 3");
    }
    let num_direct_distance_codes = 16u32 + ndirect as u32;
    let max_backward_distance = 1u32 << 22; // WBITS=22 default
    let max_distance = max_backward_distance.max(num_direct_distance_codes);

    // ----- Per-block-type CONTEXT_MODE (literal only) -----
    let mut context_modes = Vec::with_capacity(lit_bt.num_block_types as usize);
    for _ in 0..lit_bt.num_block_types {
        let mode = br.read_bits(2);
        let mode = match mode {
            0 => ContextMode::Lsb6,
            1 => ContextMode::Msb6,
            2 => ContextMode::Utf8,
            3 => ContextMode::Signed,
            _ => unreachable!(),
        };
        context_modes.push(mode);
    }
    let _ = &context_modes;

    // ----- Literal context map (RFC 7932 §9.6) -----
    let lit_cm_size = (lit_bt.num_block_types as usize) << K_LITERAL_CONTEXT_BITS;
    let (lit_context_map, ntreesl, p) =
        read_context_map(data, br.bit_pos(), lit_cm_size, 0)?;
    br.bit_pos = p;

    // ----- Distance context map (§9.6) -----
    let dist_cm_size = (dist_bt.num_block_types as usize) << K_DISTANCE_CONTEXT_BITS;
    let (mut dist_context_map, ntreesd, p) =
        read_context_map(data, br.bit_pos(), dist_cm_size, 0)?;
    br.bit_pos = p;

    // For trivial distance context (no MTF), the LSB of each entry is
    // inverted per RFC 7932 §9.6.
    if dist_bt.num_block_types == 1 {
        for entry in &mut dist_context_map {
            *entry ^= 1;
        }
    }

    // ----- Huffman tree groups -----
    let (lit_trees, p) = read_tree_group(data, br.bit_pos(), 256, ntreesl)?;
    br.bit_pos = p;
    let (cmd_trees, p) = read_tree_group(data, br.bit_pos(), 704, cmd_bt.num_block_types)?;
    br.bit_pos = p;
    let dist_alphabet_size = 16usize + ndirect + (16 << (npostfix + 1));
    let (dist_trees, p) = read_tree_group(
        data,
        br.bit_pos(),
        dist_alphabet_size.max(64),
        ntreesd,
    )?;
    br.bit_pos = p;

    // ----- Command loop -----
    let mut output: Vec<u8> = Vec::with_capacity(mlen);
    let mut dist_rb: [u32; 4] = [16, 15, 11, 4];
    let mut dist_rb_idx: i32 = 0;
    let mut p1: u8 = 0;
    let mut p2: u8 = 0;

    // Initialise block lengths. For categories with num_block_types==1,
    // the block length is the entire metablock (no switches emitted).
    if lit_bt.num_block_types == 1 { lit_bt.block_length = mlen as u32; }
    if cmd_bt.num_block_types == 1 { cmd_bt.block_length = u32::MAX; } // bound by metablock loop
    if dist_bt.num_block_types == 1 { dist_bt.block_length = u32::MAX; }

    let mut lit_block_type = lit_bt.block_type_rb[1] as usize;
    let mut cmd_block_type = cmd_bt.block_type_rb[1] as usize;
    let mut dist_block_type = dist_bt.block_type_rb[1] as usize;

    while output.len() < mlen {
        // Block-switch handling for insert-copy category.
        if cmd_bt.num_block_types > 1 && cmd_bt.block_length == 0 {
            cmd_block_type = cmd_bt.decode_switch(&mut br)? as usize;
        }

        // Read command symbol from the current command tree.
        let cmd_tree = &cmd_trees[cmd_block_type];
        let cmd_code = cmd_tree.read_symbol(&mut br).ok_or("invalid command symbol")? as usize;
        let v = &crate::prefix::kCmdLut[cmd_code];

        let insert_len_extra = if v.insert_len_extra_bits > 0 {
            br.read_bits(u32::from(v.insert_len_extra_bits))
        } else { 0 };
        let copy_extra = if v.copy_len_extra_bits > 0 {
            br.read_bits(u32::from(v.copy_len_extra_bits))
        } else { 0 };
        let insert_len = usize::from(v.insert_len_offset) + insert_len_extra as usize;
        let copy_len = usize::from(v.copy_len_offset) + copy_extra as usize;

        // Read literals.
        for _ in 0..insert_len {
            // Compute literal context.
            let context_id = context_modes[lit_block_type].context_id_2(p1, p2);
            let lit_tree_idx = lit_context_map[(lit_block_type << K_LITERAL_CONTEXT_BITS) as usize
                + context_id as usize] as usize;
            let lit_tree = &lit_trees[lit_tree_idx];
            let lit = lit_tree.read_symbol(&mut br).ok_or("invalid literal")?;
            output.push(lit as u8);
            p2 = p1;
            p1 = lit as u8;

            // Block-switch on literal block length.
            if lit_bt.num_block_types > 1 {
                lit_bt.block_length -= 1;
                if lit_bt.block_length == 0 {
                    lit_block_type = lit_bt.decode_switch(&mut br)? as usize;
                }
            }
        }

        // Metablock-end short-circuit (final INSERT-only command).
        if output.len() >= mlen {
            break;
        }

        // Distance computation.
        let distance: u32 = if v.distance_code >= 0 {
            crate::decoder::take_distance_from_ring_buffer(
                v.distance_code as i32, &mut dist_rb, &mut dist_rb_idx,
            )
        } else {
            // Read distance code from distance tree.
            let dist_context = if v.copy_len_offset == 0 { 1 } else { 0 };
            let dist_tree_idx = dist_context_map[(dist_block_type << K_DISTANCE_CONTEXT_BITS) as usize
                + dist_context] as usize;
            let dist_tree = &dist_trees[dist_tree_idx];
            let dist_code = dist_tree.read_symbol(&mut br).ok_or("invalid distance symbol")? as i32;
            crate::decoder::decode_distance_from_code(
                dist_code,
                num_direct_distance_codes,
                npostfix as i32,
                &mut br,
                &mut dist_rb,
                &mut dist_rb_idx,
            )
        };

        // Static dictionary reference vs LZ77 back-reference.
        if (distance as i32) > max_distance as i32 {
            if dictionary_lookup(&mut output, copy_len as u32, distance as i32, max_distance)
                .is_none()
            {
                return Err("static dictionary not supported");
            }
        } else {
            if distance == 0 || distance as usize > output.len() {
                return Err("invalid back-reference distance");
            }
            let src = output.len() - distance as usize;
            for i in 0..copy_len {
                let b = output[src + i];
                output.push(b);
            }
        }

        // Update p1/p2 from copied bytes.
        if copy_len > 0 {
            let last = output[output.len() - 1];
            p2 = if copy_len > 1 { output[output.len() - 2] } else { p1 };
            p1 = last;
        }

        // Block-switch on distance block length.
        if dist_bt.num_block_types > 1 && copy_len > 0 {
            dist_bt.block_length -= 1;
            if dist_bt.block_length == 0 {
                dist_block_type = dist_bt.decode_switch(&mut br)? as usize;
            }
        }

        if output.len() > mlen + 1 {
            return Err("metablock overran mlen");
        }
    }

    Ok((br.bit_pos(), output))
}

