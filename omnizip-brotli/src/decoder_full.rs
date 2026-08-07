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

use crate::decoder::{read_huffman_table, read_varlen_uint8, BitReader, ContextMode, HuffmanTable};
use crate::prefix::kBlockLengthPrefixCode;

/// Number of distance context bits (RFC 7932 §10.4 + upstream
/// `BROTLI_DISTANCE_CONTEXT_BITS`). Distance context is computed from
/// the copy length's category (4 buckets: <=2, 3-4, 5-8, >=9), so
/// only 2 bits are needed to index the distance context map.
const K_DISTANCE_CONTEXT_BITS: u32 = 2;

/// Number of literal context bits (RFC 7932 §10.1 + upstream
/// `BROTLI_LITERAL_CONTEXT_BITS`). Literal context uses p1, p2 (top
/// 6 bits each) → 64 contexts.
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
    /// 3. Initial block LENGTH only (Huffman symbol + extra bits via
    ///    `kBlockLengthPrefixCode`). The initial block TYPE defaults
    ///    to `block_type_rb[1] = 0` — upstream never reads the initial
    ///    block type from the bitstream (see `PrepareLiteralDecoding`).
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

        // Initial block length via the block-length tree.
        // The initial block type stays at block_type_rb[1] = 0.
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
/// `num_htrees` is passed in (already read by the caller via
/// `read_varlen_uint8`). This function starts at the RLE flag.
///
/// Layout when `num_htrees <= 1`: zero-filled context map, no data.
/// Layout when `num_htrees > 1`: optional RLE flag (1 bit), then
/// context-map code Huffman tree (alphabet `max_rle + num_htrees`),
/// then the per-context entries with optional run-length encoding
/// of zeros, then an optional inverse-MTF flag (1 bit).
pub(crate) fn read_context_map(
    data: &[u8],
    bit_pos: usize,
    context_map_size: usize,
    num_htrees: u32,
    max_rle_override: u32,
) -> Result<(Vec<u8>, usize), &'static str> {
    let mut br = BitReader::new(data);
    br.bit_pos = bit_pos;

    let mut context_map = vec![0u8; context_map_size];

    if num_htrees <= 1 {
        // Trivial: every context maps to tree 0.
        return Ok((context_map, br.bit_pos()));
    }

    // RLE flag (RFC 7932 §9.6). Upstream reads 5 bits as a peek:
    //   bits = PeekBits(5);
    //   if (bits & 1) != 0 {
    //       max_run_length_prefix = (bits >> 1) + 1;  // bits 1..4 + 1
    //       DropBits(5);
    //   } else {
    //       max_run_length_prefix = 0;
    //       DropBits(1);
    //   }
    // The 1-bit RLE flag is bit 0; bits 1..4 are the max_rle - 1 value.
    let max_run_length_prefix;
    if br.read_bits(1) != 0 {
        // RLE flag set: read 4 more bits for max_run_length_prefix - 1.
        max_run_length_prefix = br.read_bits(4) + 1;
    } else {
        max_run_length_prefix = 0;
    }
    let max_rle = max_run_length_prefix.max(max_rle_override);

    // Context-map code Huffman tree.
    let alphabet_size = max_rle + num_htrees;
    let (cm_tree, p) = read_huffman_table(data, br.bit_pos(), alphabet_size as usize)?;
    br.bit_pos = p;

    // Read context-map entries.
    let mut i = 0usize;
    while i < context_map_size {
        let code = cm_tree
            .read_symbol(&mut br)
            .ok_or("invalid context-map symbol")?;
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

    Ok((context_map, br.bit_pos()))
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
/// Resolve a static dictionary reference (RFC 7932 §10.4) via the
/// shared `dictionary` module. Returns `Some(())` if the reference is
/// valid; `None` if the word length or transform index is out of range.
fn dictionary_lookup(
    output: &mut Vec<u8>,
    copy_len: u32,
    distance_code: i32,
    max_distance: u32,
) -> Option<()> {
    crate::dictionary::dictionary_lookup(output, copy_len, distance_code, max_distance)
}

/// Decode a Huffman-coded metablock using the full RFC 7932 grammar
/// (block-type machinery, context maps, multi-tree groups, context
/// modes, static dictionary).
///
/// Caller is `decode()` in `decoder.rs`, which dispatches here when
/// any category has `num_block_types > 1`. `bit_pos` points to the
/// block-type code tree section (after NBLTYPES reads).
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

    // Per-category block-type code reading. Caller has already consumed
    // the NBLTYPES DecodeVarLenUint8 field.
    let mut lit_bt = BlockTypeState::default();
    lit_bt.num_block_types = nbltypesl;
    br.bit_pos = lit_bt.read_block_type_trees(data, br.bit_pos())?;
    let mut cmd_bt = BlockTypeState::default();
    cmd_bt.num_block_types = nbltypesc;
    br.bit_pos = cmd_bt.read_block_type_trees(data, br.bit_pos())?;
    let mut dist_bt = BlockTypeState::default();
    dist_bt.num_block_types = nbltypesd;
    br.bit_pos = dist_bt.read_block_type_trees(data, br.bit_pos())?;

    // NPOSTFIX + NDIRECT.
    let npostfix = br.read_bits(2) as usize;
    let ndirect_raw = br.read_bits(4) as usize;
    // Per RFC 7932 §9.4: NDIRECT = NDMOEM << NPOSTFIX.
    let _ndirect = ndirect_raw << npostfix;

    // Per-block-type CONTEXT_MODE (literal only).
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

    // NTREESL and NTREESD are NOT read here — they're interleaved with
    // the context map reads inside finish_metablock_decode (per upstream:
    // NTREESL is read first, then literal context map, then NTREESD,
    // then distance context map).

    finish_metablock_decode(
        data,
        &mut br,
        mlen,
        npostfix,
        ndirect_raw,
        None,
        None, // NTREES read inline (NBLTYPES > 1 path)
        lit_bt,
        cmd_bt,
        dist_bt,
        context_modes,
    )
}

/// Dispatch from trivial path when NTREES > 1 (multi-tree Huffman
/// groups with context maps) but NBLTYPES = 1 for all categories.
///
/// Caller has already consumed NBLTYPES (all = 1), NPOSTFIX, NDMOEM,
/// CONTEXT_MODE, NTREESL, NTREESD. `bit_pos` points to the literal
/// context map.
pub(crate) fn decode_compressed_metablock_full_with_trees(
    data: &[u8],
    bit_pos: usize,
    mlen: usize,
    npostfix: usize,
    ndirect_raw: usize,
    context_mode_bits: u32,
    ntreesl: u32,
    ntreesd: Option<u32>,
) -> Result<(usize, Vec<u8>), &'static str> {
    let mut br = BitReader::new(data);
    br.bit_pos = bit_pos;

    // NBLTYPES = 1 for all categories, so no block-type trees.
    let lit_bt = BlockTypeState::default();
    let cmd_bt = BlockTypeState::default();
    let dist_bt = BlockTypeState::default();

    // Single CONTEXT_MODE field (NBLTYPESL = 1).
    let mode = match context_mode_bits {
        0 => ContextMode::Lsb6,
        1 => ContextMode::Msb6,
        2 => ContextMode::Utf8,
        3 => ContextMode::Signed,
        _ => unreachable!(),
    };
    let context_modes = vec![mode];

    finish_metablock_decode(
        data,
        &mut br,
        mlen,
        npostfix,
        ndirect_raw,
        Some(ntreesl),
        ntreesd, // NTREES_L already read; NTREES_D may be inline
        lit_bt,
        cmd_bt,
        dist_bt,
        context_modes,
    )
}

/// Shared tail of the two full-path entry points: reads context maps,
/// Huffman tree groups, then runs the command loop.
#[allow(clippy::too_many_lines)]
fn finish_metablock_decode(
    data: &[u8],
    mut br: &mut BitReader,
    mlen: usize,
    npostfix: usize,
    ndirect_raw: usize,
    ntreesl_opt: Option<u32>,
    ntreesd_opt: Option<u32>,
    mut lit_bt: BlockTypeState,
    mut cmd_bt: BlockTypeState,
    mut dist_bt: BlockTypeState,
    context_modes: Vec<ContextMode>,
) -> Result<(usize, Vec<u8>), &'static str> {
    let ndirect = ndirect_raw << npostfix;
    if npostfix > 3 {
        return Err("invalid metablock: NPOSTFIX > 3");
    }
    let num_direct_distance_codes = 16u32 + ndirect as u32;
    // Per RFC 7932 §9.1: max_backward_distance = (1 << WBITS) - WINDOW_GAP.
    // WBITS=22 default; WINDOW_GAP = 0x8000 (32 KB).
    let max_backward_distance: u32 = (1u32 << 22).saturating_sub(0x8000);

    // ----- Literal context map (RFC 7932 §9.6) -----
    let lit_cm_size = (lit_bt.num_block_types as usize) << K_LITERAL_CONTEXT_BITS;
    let ntreesl = match ntreesl_opt {
        Some(v) => v,
        None => read_varlen_uint8(&mut br)? + 1,
    };
    let (lit_context_map, p) = read_context_map(data, br.bit_pos(), lit_cm_size, ntreesl, 0)?;
    br.bit_pos = p;

    // ----- Distance context map (§9.6) -----
    let dist_cm_size = (dist_bt.num_block_types as usize) << K_DISTANCE_CONTEXT_BITS;
    let ntreesd = match ntreesd_opt {
        Some(v) => v,
        None => read_varlen_uint8(&mut br)? + 1,
    };
    let (dist_context_map, p) = read_context_map(data, br.bit_pos(), dist_cm_size, ntreesd, 0)?;
    br.bit_pos = p;

    // ----- Huffman tree groups -----
    let (lit_trees, p) = read_tree_group(data, br.bit_pos(), 256, ntreesl)?;
    br.bit_pos = p;
    let (cmd_trees, p) = read_tree_group(data, br.bit_pos(), 704, cmd_bt.num_block_types)?;
    br.bit_pos = p;
    let dist_alphabet_size = num_direct_distance_codes as usize + (48usize << npostfix);
    let (dist_trees, p) = read_tree_group(data, br.bit_pos(), dist_alphabet_size, ntreesd)?;
    br.bit_pos = p;

    // ----- Command loop -----
    let mut output: Vec<u8> = Vec::with_capacity(mlen);
    let mut dist_rb: [u32; 4] = [16, 15, 11, 4];
    let mut dist_rb_idx: i32 = 0;
    let mut p1: u8 = 0;
    let mut p2: u8 = 0;

    // Initialise block lengths. For categories with num_block_types==1,
    // the block length is the entire metablock (no switches emitted).
    if lit_bt.num_block_types == 1 {
        lit_bt.block_length = mlen as u32;
    }
    if cmd_bt.num_block_types == 1 {
        cmd_bt.block_length = u32::MAX;
    } // bound by metablock loop
    if dist_bt.num_block_types == 1 {
        dist_bt.block_length = u32::MAX;
    }

    let mut lit_block_type = lit_bt.block_type_rb[1] as usize;
    let mut cmd_block_type = cmd_bt.block_type_rb[1] as usize;
    let mut dist_block_type = dist_bt.block_type_rb[1] as usize;

    while output.len() < mlen {
        // Block-switch handling for insert-copy category.
        if cmd_bt.num_block_types > 1 {
            if cmd_bt.block_length == 0 {
                cmd_block_type = cmd_bt.decode_switch(&mut br)? as usize;
            }
            cmd_bt.block_length -= 1;
        }

        // Read command symbol from the current command tree.
        let cmd_tree = &cmd_trees[cmd_block_type];
        let cmd_code = cmd_tree
            .read_symbol(&mut br)
            .ok_or("invalid command symbol")? as usize;
        let v = &crate::prefix::kCmdLut[cmd_code];

        let insert_len_extra = if v.insert_len_extra_bits > 0 {
            br.read_bits(u32::from(v.insert_len_extra_bits))
        } else {
            0
        };
        let copy_extra = if v.copy_len_extra_bits > 0 {
            br.read_bits(u32::from(v.copy_len_extra_bits))
        } else {
            0
        };
        let insert_len = usize::from(v.insert_len_offset) + insert_len_extra as usize;
        let copy_len = usize::from(v.copy_len_offset) + copy_extra as usize;

        // Read literals.
        for _ in 0..insert_len {
            // Compute literal context.
            let context_id = context_modes[lit_block_type].context_id_2(p1, p2);
            let lit_tree_idx = lit_context_map
                [(lit_block_type << K_LITERAL_CONTEXT_BITS) as usize + context_id as usize]
                as usize;
            let lit_tree = &lit_trees[lit_tree_idx];
            let lit = lit_tree.read_symbol(&mut br).ok_or("invalid literal")?;
            output.push(lit as u8);
            p2 = p1;
            p1 = lit as u8;

            // Block-switch on literal block length.
            if lit_bt.num_block_types > 1 {
                if lit_bt.block_length == 0 {
                    lit_block_type = lit_bt.decode_switch(&mut br)? as usize;
                }
                lit_bt.block_length -= 1;
            }
        }

        // Metablock-end short-circuit (final INSERT-only command).
        if output.len() >= mlen {
            break;
        }

        // Distance computation.
        let distance: u32 = if v.distance_code >= 0 {
            // Implicit distance (kCmdLut.distance_code == 0):
            // use most recent from ring buffer. Matches upstream
            // CommandPostDecodeLiterals: --idx, dist_rb[idx&3].
            dist_rb_idx -= 1;
            dist_rb[(dist_rb_idx & 3) as usize]
        } else {
            // Read distance code from distance tree.
            // distance_context = v.context (per upstream ReadCommandInternal).
            let dist_context = v.context as usize;
            let dist_tree_idx = dist_context_map
                [(dist_block_type << K_DISTANCE_CONTEXT_BITS) as usize + dist_context]
                as usize;
            let dist_tree = &dist_trees[dist_tree_idx];
            let dist_code = dist_tree
                .read_symbol(&mut br)
                .ok_or("invalid distance symbol")? as i32;
            crate::decoder::decode_distance_from_code(
                dist_code,
                num_direct_distance_codes,
                npostfix as i32,
                &mut br,
                &mut dist_rb,
                &mut dist_rb_idx,
            )
        };

        // Block-switch on distance block length (after each distance code).
        if dist_bt.num_block_types > 1 {
            if dist_bt.block_length == 0 {
                dist_block_type = dist_bt.decode_switch(&mut br)? as usize;
            }
            dist_bt.block_length -= 1;
        }

        // Per upstream: max_distance = min(pos, max_backward_distance).
        let pos = output.len() as u32;
        let max_distance = if pos < max_backward_distance {
            pos
        } else {
            max_backward_distance
        };

        // Static dictionary reference vs LZ77 back-reference.
        if (distance as i32) > max_distance as i32 {
            if dictionary_lookup(&mut output, copy_len as u32, distance as i32, max_distance)
                .is_none()
            {
                return Err("static dictionary not supported");
            }
            // Compensate dist_rb_idx for implicit distance path (which
            // decremented idx; the dictionary path doesn't write back).
            if v.distance_code >= 0 {
                dist_rb_idx = dist_rb_idx.wrapping_add(1);
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
            // Update recent-distances cache (upstream LZ77 copy path).
            dist_rb[(dist_rb_idx & 3) as usize] = distance;
            dist_rb_idx = dist_rb_idx.wrapping_add(1);
        }

        // Update p1/p2 from copied bytes.
        if copy_len > 0 {
            let last = output[output.len() - 1];
            p2 = if copy_len > 1 {
                output[output.len() - 2]
            } else {
                p1
            };
            p1 = last;
        }

        // (Distance block switch handled above after distance read.)

        if output.len() > mlen + 1 {
            return Err("metablock overran mlen");
        }
    }

    Ok((br.bit_pos(), output))
}
