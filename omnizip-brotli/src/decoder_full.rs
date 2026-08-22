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
use crate::prefix::{kBlockLengthPrefixCode, kCmdLut};

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
        br.set_bit_pos(bit_pos);

        self.block_type_rb = [1, 0];
        if self.num_block_types == 1 {
            return Ok(br.bit_pos());
        }

        // Block-type code tree: alphabet size 2 + NBLTYPES.
        let alphabet_size = 2 + self.num_block_types;
        let (tree, p) = read_huffman_table(data, br.bit_pos(), alphabet_size as usize)?;
        self.block_type_tree = Some(tree);
        br.set_bit_pos(p);

        // Block-length code tree: alphabet size 26 (kBlockLengthPrefixCode).
        let (tree, p) = read_huffman_table(data, br.bit_pos(), 26)?;
        self.block_len_tree = Some(tree);
        br.set_bit_pos(p);

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
    let extra = br.read_bits(u32::from(entry.nbits));
    Ok(u32::from(entry.offset) + extra)
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
    br.set_bit_pos(bit_pos);

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
    br.set_bit_pos(p);

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

/// Diagnostic command-stream statistics (env-gated via BROTLI_DEC_STATS).
#[derive(Default)]
struct DecStats {
    cmds: u64,
    literal_count: u64,
    ins_extra: u64,
    cpy_extra: u64,
    cmd_hists: std::collections::BTreeMap<usize, [u32; 704]>,
    lit_hists: std::collections::BTreeMap<usize, [u32; 256]>,
    dist_hists: std::collections::BTreeMap<usize, std::collections::BTreeMap<u32, u32>>,
    copies: u64,
    copy_bytes: u64,
    max_copy: u64,
    implicit: u64,
    dist_hist: std::collections::BTreeMap<&'static str, u64>,
    last_dists: std::collections::VecDeque<u32>,
    recent_cmds: std::collections::VecDeque<(u32, u32, u32)>,
    dist_code_hist: std::collections::BTreeMap<i32, u64>,
    npostfix: u32,
    ndirect_raw: u32,
    mb_bounds: Vec<(usize, usize)>,
    // (pos_start, ins, copy, dist_sym (-1=implicit), dist, implicit, rb_before, rb_idx_before)
    trace: Vec<(usize, u32, u32, i32, u32, bool, [u32; 4], i32)>,
    at: u64,
    // Full per-command dump (BROTLI_DEC_STATS + BROTLI_DEC_CMDDUMP):
    // (out_pos, insert_len, copy_len, distance).
    cmd_dump: Vec<(usize, u32, u32, u32)>,
}

static DEC_STATS: std::sync::Mutex<DecStats> = std::sync::Mutex::new(DecStats::empty());

impl DecStats {
    const fn empty() -> Self {
        Self {
            cmds: 0,
            literal_count: 0,
            ins_extra: 0,
            cpy_extra: 0,
            cmd_hists: std::collections::BTreeMap::new(),
            lit_hists: std::collections::BTreeMap::new(),
            dist_hists: std::collections::BTreeMap::new(),
            copies: 0,
            copy_bytes: 0,
            max_copy: 0,
            implicit: 0,
            dist_hist: std::collections::BTreeMap::new(),
            last_dists: std::collections::VecDeque::new(),
            recent_cmds: std::collections::VecDeque::new(),
            dist_code_hist: std::collections::BTreeMap::new(),
            npostfix: 0,
            ndirect_raw: 0,
            mb_bounds: Vec::new(),
            trace: Vec::new(),
            at: u64::MAX,
            cmd_dump: Vec::new(),
        }
    }
}

fn dec_stats() -> std::sync::MutexGuard<'static, DecStats> {
    DEC_STATS.lock().unwrap_or_else(|e| e.into_inner())
}

// Hot-path env gates cached once: getenv takes a global lock and walks
// environ on every call — in the per-command loop it was ~70% of decode
// time (sampled).
static DEC_STATS_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
fn dec_stats_on() -> bool {
    *DEC_STATS_ON.get_or_init(|| std::env::var("BROTLI_DEC_STATS").is_ok())
}

static DBG_DC_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
fn dbg_dc_on() -> bool {
    *DBG_DC_ON.get_or_init(|| std::env::var("BROTLI_DBG_DC").is_ok())
}

static CMD_DUMP_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
fn cmd_dump_on() -> bool {
    *CMD_DUMP_ON.get_or_init(|| std::env::var("BROTLI_DEC_CMDDUMP").is_ok())
}

static TRACE_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
fn trace_on() -> bool {
    *TRACE_ON.get_or_init(|| {
        std::env::var("BROTLI_DEC_AT").is_ok() || std::env::var("BROTLI_DEC_STATS").is_ok()
    })
}

/// LZ77 back-reference copy: replicate the span at `src` for `len`
/// bytes, handling overlap (distance < len) by doubling the copied
/// window. `extend_from_within` is a bulk memcpy vs the old byte loop.
fn lz77_copy(output: &mut Vec<u8>, src: usize, len: usize) {
    let mut remaining = len;
    while remaining > 0 {
        let take = remaining.min(output.len() - src);
        output.extend_from_within(src..src + take);
        remaining -= take;
    }
}

/// Print the accumulated decoder command statistics (diagnostic).
#[doc(hidden)]
pub fn _print_dec_stats(total_input: usize) {
    let st = dec_stats();
    if let (Ok(path), true) = (std::env::var("BROTLI_DEC_CMDDUMP"), !st.cmd_dump.is_empty()) {
        let mut body = String::with_capacity(st.cmd_dump.len() * 24);
        for (pos, ins, cpy, dist) in &st.cmd_dump {
            body.push_str(&format!("{pos} {ins} {cpy} {dist}\n"));
        }
        let _ = std::fs::write(&path, body);
        eprintln!("DEC_STATS cmd dump: {} cmds -> {path}", st.cmd_dump.len());
    }
    if std::env::var("BROTLI_DEC_CMDSYM").is_ok() {
        let mut agg = [0u64; 704];
        for (_bt, h) in st.cmd_hists.iter() {
            for (sym, &f) in h.iter().enumerate() {
                agg[sym] += u64::from(f);
            }
        }
        let mut top: Vec<(usize, u64)> = agg
            .iter()
            .enumerate()
            .filter(|&(_, &f)| f > 0)
            .map(|(s, &f)| (s, f))
            .collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        let mut out = String::from("DEC_STATS top cmd syms (ins_off,cpy_off,dc -> count):");
        for (sym, f) in top.iter().take(16) {
            let e = &kCmdLut[*sym];
            out.push_str(&format!(
                " ({},{},{}->{})",
                e.insert_len_offset, e.copy_len_offset, e.distance_code, f
            ));
        }
        eprintln!("{out}");
        eprintln!("DEC_STATS distinct cmd syms: {}", top.len());
    }
    eprintln!(
        "DEC_STATS: cmds={} copies={} (implicit-rep0: {}) literals={} | avg_copy={:.1} max_copy={} copy_pct={:.1}%",
        st.cmds,
        st.copies,
        st.implicit,
        st.literal_count,
        if st.copies > 0 { st.copy_bytes as f64 / st.copies as f64 } else { 0.0 },
        st.max_copy,
        st.copy_bytes as f64 * 100.0 / total_input as f64,
    );
    eprintln!(
        "DEC_STATS extras: ins_extra={} cpy_extra={}",
        st.ins_extra, st.cpy_extra
    );
    {
        let mut bits = 0.0f64;
        for (_bt, h) in st.cmd_hists.iter() {
            let t: u64 = h.iter().map(|&x| u64::from(x)).sum();
            if t == 0 {
                continue;
            }
            for &f in h.iter() {
                if f > 0 {
                    bits -= f as f64 * (f as f64 / t as f64).log2();
                }
            }
        }
        let mut lbits = 0.0f64;
        for (_t, h) in st.lit_hists.iter() {
            let t: u64 = h.iter().map(|&x| u64::from(x)).sum();
            if t == 0 {
                continue;
            }
            for &f in h.iter() {
                if f > 0 {
                    lbits -= f as f64 * (f as f64 / t as f64).log2();
                }
            }
        }
        let mut dbits = 0.0f64;
        let mut dtot = 0u64;
        for (_t, h) in st.dist_hists.iter() {
            let t: u64 = h.values().map(|&x| u64::from(x)).sum();
            dtot += t;
            if t == 0 {
                continue;
            }
            for &f in h.values() {
                if f > 0 {
                    dbits -= f as f64 * (f as f64 / t as f64).log2();
                }
            }
        }
        eprintln!(
            "DEC_STATS entropy: cmd_sym={:.0} lit_sym={:.0} dist_sym={:.0} (n={}) cmd_blocks={} lit_trees={} dist_trees={}",
            bits, lbits, dbits, dtot, st.cmd_hists.len(), st.lit_hists.len(), st.dist_hists.len()
        );
        let mut per_tree: Vec<(usize, u64, usize)> = st
            .lit_hists
            .iter()
            .map(|(t, h)| {
                (
                    *t,
                    h.iter().map(|&x| u64::from(x)).sum(),
                    h.iter().filter(|&&x| x > 0).count(),
                )
            })
            .collect();
        per_tree.sort_by_key(|&(_, c, _)| std::cmp::Reverse(c));
        for (t, c, d) in per_tree.iter().take(24) {
            eprintln!("DEC_STATS lit_tree[{t}]: count={c} distinct={d}");
        }
    }
    eprintln!("DEC_STATS distances: {:?}", st.dist_hist);
    eprintln!("DEC_STATS last dists: {:?}", st.last_dists);
    let mut dch: Vec<(i32, u64)> = st.dist_code_hist.iter().map(|(&k, &v)| (k, v)).collect();
    dch.sort_by_key(|&(_k, c)| std::cmp::Reverse(c));
    dch.truncate(10);
    eprintln!(
        "DEC_STATS top dist codes (npostfix={}, ndirect_raw={}): {:?}",
        st.npostfix, st.ndirect_raw, dch
    );
    // Print a window of mid-stream commands (rows 2000-2040 of the buffer).
    let start = st.recent_cmds.len().saturating_sub(2000);
    for (k, (ins, copy, dist)) in st.recent_cmds.iter().enumerate().skip(start).take(40) {
        eprintln!("DEC_STATS cmd[{k}]: ins={ins} copy={copy} dist={dist}");
    }
    eprintln!("DEC_STATS mb_bounds: {:?}", st.mb_bounds);
    if st.at != u64::MAX {
        for (pos, ins, copy, sym, dist, implicit, rb, rb_idx) in st.trace.iter() {
            eprintln!(
                "DEC_TRACE pos={pos} ins={ins} copy={copy} sym={sym} dist={dist} implicit={implicit} rb={rb:?} rb_idx={rb_idx}"
            );
        }
    }
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
    nbltypesc: Option<u32>,
    nbltypesd: Option<u32>,
    output_base: usize,
    max_backward_distance: u32,
    prior_output: &[u8],
    ctx_in: (u8, u8),
    dist_rb: &mut [u32; 4],
    dist_rb_idx: &mut i32,
) -> Result<(usize, Vec<u8>, (u8, u8)), &'static str> {
    let mut br = BitReader::new(data);
    br.set_bit_pos(bit_pos);

    // Per upstream `BROTLI_STATE_HUFFMAN_CODE_0..3`: NBLTYPES values
    // are interleaved with their block-type trees. Caller has already
    // read NBLTYPES_L (and optionally NBLTYPES_C / NBLTYPES_D); we
    // read each category's block-type trees after the NBLTYPES value
    // is known, then read the next NBLTYPES inline if not yet read.
    let mut lit_bt = BlockTypeState::default();
    lit_bt.num_block_types = nbltypesl;
    br.set_bit_pos(lit_bt.read_block_type_trees(data, br.bit_pos())?);

    let nbltypesc = match nbltypesc {
        Some(v) => v,
        None => crate::decoder::read_varlen_uint8(&mut br)? + 1,
    };
    let mut cmd_bt = BlockTypeState::default();
    cmd_bt.num_block_types = nbltypesc;
    br.set_bit_pos(cmd_bt.read_block_type_trees(data, br.bit_pos())?);

    let nbltypesd = match nbltypesd {
        Some(v) => v,
        None => crate::decoder::read_varlen_uint8(&mut br)? + 1,
    };
    let mut dist_bt = BlockTypeState::default();
    dist_bt.num_block_types = nbltypesd;
    br.set_bit_pos(dist_bt.read_block_type_trees(data, br.bit_pos())?);

    // NPOSTFIX + NDIRECT.
    let npostfix = br.read_bits(2) as usize;
    let ndirect_raw = br.read_bits(4) as usize;
    if std::env::var("BROTLI_DEC_STATS").is_ok() {
        let mut st = dec_stats();
        st.npostfix = npostfix as u32;
        st.ndirect_raw = ndirect_raw as u32;
    }
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
        output_base,
        max_backward_distance,
        prior_output,
        ctx_in,
        dist_rb,
        dist_rb_idx,
    )
}

/// Dispatch from trivial path when NTREES > 1 (multi-tree Huffman
/// groups with context maps) but NBLTYPES = 1 for all categories.
///
/// Caller has already consumed NBLTYPES (all = 1), NPOSTFIX, NDMOEM,
/// `CONTEXT_MODE`, NTREESL, NTREESD. `bit_pos` points to the literal
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
    output_base: usize,
    max_backward_distance: u32,
    prior_output: &[u8],
    ctx_in: (u8, u8),
    dist_rb: &mut [u32; 4],
    dist_rb_idx: &mut i32,
) -> Result<(usize, Vec<u8>, (u8, u8)), &'static str> {
    let mut br = BitReader::new(data);
    br.set_bit_pos(bit_pos);

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
        output_base,
        max_backward_distance,
        prior_output,
        ctx_in,
        dist_rb,
        dist_rb_idx,
    )
}

/// Shared tail of the two full-path entry points: reads context maps,
/// Huffman tree groups, then runs the command loop.
#[allow(clippy::too_many_lines)]
fn finish_metablock_decode(
    data: &[u8],
    br: &mut BitReader,
    mlen: usize,
    npostfix: usize,
    ndirect_raw: usize,
    ntreesl_opt: Option<u32>,
    ntreesd_opt: Option<u32>,
    mut lit_bt: BlockTypeState,
    mut cmd_bt: BlockTypeState,
    mut dist_bt: BlockTypeState,
    context_modes: Vec<ContextMode>,
    output_base: usize,
    max_backward_distance: u32,
    prior_output: &[u8],
    ctx_in: (u8, u8),
    dist_rb: &mut [u32; 4],
    dist_rb_idx: &mut i32,
) -> Result<(usize, Vec<u8>, (u8, u8)), &'static str> {
    if let Ok(v) = std::env::var("BROTLI_DEC_AT") {
        let mut st = dec_stats();
        if st.at == u64::MAX {
            st.at = v.parse().unwrap_or(u64::MAX);
        }
        st.mb_bounds.push((output_base, mlen));
    }
    let ndirect = ndirect_raw << npostfix;
    if npostfix > 3 {
        return Err("invalid metablock: NPOSTFIX > 3");
    }
    if std::env::var("BROTLI_DICT_DEBUG").is_ok() {
        eprintln!(
            "MB output_base={output_base} mlen={mlen} nbltypesl={} nbltypesc={} nbltypesd={}",
            lit_bt.num_block_types, cmd_bt.num_block_types, dist_bt.num_block_types
        );
    }
    let num_direct_distance_codes = 16u32 + ndirect as u32;

    // ----- Literal context map (RFC 7932 §9.6) -----
    let lit_cm_size = (lit_bt.num_block_types as usize) << K_LITERAL_CONTEXT_BITS;
    let ntreesl = match ntreesl_opt {
        Some(v) => v,
        None => read_varlen_uint8(br)? + 1,
    };
    let (lit_context_map, p) = read_context_map(data, br.bit_pos(), lit_cm_size, ntreesl, 0)?;
    br.set_bit_pos(p);

    // ----- Distance context map (§9.6) -----
    let dist_cm_size = (dist_bt.num_block_types as usize) << K_DISTANCE_CONTEXT_BITS;
    let ntreesd = match ntreesd_opt {
        Some(v) => v,
        None => read_varlen_uint8(br)? + 1,
    };
    let (dist_context_map, p) = read_context_map(data, br.bit_pos(), dist_cm_size, ntreesd, 0)?;
    br.set_bit_pos(p);
    if std::env::var("BROTLI_DBG_DC").is_ok() {
        eprintln!("DCDBG ntreesd={ntreesd} cm_size={dist_cm_size} map={dist_context_map:?}");
    }

    // ----- Huffman tree groups -----
    let t_dbg = std::env::var("BROTLI_DEC_TIMER").is_ok();
    let t0 = std::time::Instant::now();
    let (lit_trees, p) = read_tree_group(data, br.bit_pos(), 256, ntreesl)?;
    br.set_bit_pos(p);
    let (cmd_trees, p) = read_tree_group(data, br.bit_pos(), 704, cmd_bt.num_block_types)?;
    br.set_bit_pos(p);
    let dist_alphabet_size = num_direct_distance_codes as usize + (48usize << npostfix);
    let (dist_trees, p) = read_tree_group(data, br.bit_pos(), dist_alphabet_size, ntreesd)?;
    br.set_bit_pos(p);
    let t_trees = t0.elapsed();

    // ----- Command loop -----
    let mut output: Vec<u8> = Vec::with_capacity(mlen);
    // `dist_rb` / `dist_rb_idx` are frame-scoped state owned by the
    // caller: the recent-distances ring persists ACROSS metablocks
    // (upstream keeps one ring per stream). Reinitializing per
    // metablock broke mid-frame rep codes on multi-metablock streams.

    let (mut p1, mut p2) = ctx_in;

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
    let mut lit_count_total = 0usize;
    let mut dec_cmd_n = 0usize;
    let mut cmd_block_type = cmd_bt.block_type_rb[1] as usize;
    let mut dist_block_type = dist_bt.block_type_rb[1] as usize;

    // Hot-loop diagnostics flags hoisted to locals: the OnceLock
    // deref is an atomic load per literal otherwise.
    let stats_flag = dec_stats_on();
    let dbg_dc_flag = dbg_dc_on();
    let trace_flag = trace_on();
    while output.len() < mlen {
        // Block-switch handling for insert-copy category.
        if cmd_bt.num_block_types > 1 {
            if cmd_bt.block_length == 0 {
                cmd_block_type = cmd_bt.decode_switch(br)? as usize;
                if std::env::var("BROTLI_SWITCH_LOG").is_ok() {
                    eprintln!(
                        "DECSW-CMD n={} pos={} type={} len={}",
                        dec_cmd_n,
                        output_base + output.len(),
                        cmd_block_type,
                        cmd_bt.block_length
                    );
                }
            }
            cmd_bt.block_length -= 1;
        }

        dec_cmd_n += 1;
        // Read command symbol from the current command tree.
        let cmd_tree = &cmd_trees[cmd_block_type];
        let cmd_code = cmd_tree.read_symbol(br).ok_or("invalid command symbol")? as usize;
        if std::env::var("BROTLI_SYM_TRACE").is_ok() {
            static SYM_N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = SYM_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if (230..=240).contains(&n) {
                eprintln!("DECSYM {n} -> sym={cmd_code}");
            }
        }
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

        let cmd_start_outlen = output.len();
        if std::env::var("BROTLI_CMD_TRACE").is_ok() {
            eprintln!(
                "DECCMD ins={insert_len} cpy={copy_len} outlen={}",
                output.len()
            );
        }
        // Read literals.
        for _ in 0..insert_len {
            // Metablock boundary: per upstream `ProcessCommandsInternal`,
            // the literal loop must stop as soon as we've emitted mlen
            // bytes (a final INSERT-only command can have insert_len >
            // remaining metablock length).
            if output.len() >= mlen {
                break;
            }

            // Block-switch on literal block length (BEFORE reading the
            // literal, per upstream `ProcessCommandsInternal`).
            if std::env::var("BROTLI_SW_TRACE").is_ok() {
                lit_count_total += 1;
            }
            if lit_bt.num_block_types > 1 {
                if lit_bt.block_length == 0 {
                    lit_block_type = lit_bt.decode_switch(br)? as usize;
                    if std::env::var("BROTLI_SW_TRACE").is_ok() {
                        eprintln!(
                            "DECSW out={} type={lit_block_type} len={} lit={lit_count_total} bit={}",
                            output.len(),
                            lit_bt.block_length,
                            br.bit_pos()
                        );
                    }
                }
                lit_bt.block_length -= 1;
            }

            // Compute literal context.
            let context_id = context_modes[lit_block_type].context_id_2(p1, p2);
            let lit_tree_idx = lit_context_map
                [(lit_block_type << K_LITERAL_CONTEXT_BITS) + context_id as usize]
                as usize;
            let lit_tree = &lit_trees[lit_tree_idx];
            let lit = lit_tree.read_symbol(br).ok_or("invalid literal")?;
            if std::env::var("BROTLI_LIT_TRACE").is_ok() {
                eprintln!(
                    "DECLIT {lit_count_total} bit={} tree={lit_tree_idx} byte={lit} p1={} p2={} blk={lit_block_type}",
                    br.bit_pos(),
                    u8::from(p1),
                    u8::from(p2)
                );
            }
            if stats_flag {
                let mut st = dec_stats();
                st.lit_hists.entry(lit_tree_idx).or_insert([0u32; 256])[lit as usize] += 1;
            }
            output.push(lit as u8);
            p2 = p1;
            p1 = lit as u8;
        }

        // Metablock-end short-circuit (final INSERT-only command).
        if output.len() >= mlen {
            break;
        }

        let mut dist_sym: i32 = -1;
        // Distance computation.
        // Per upstream `ReadDistanceInternal`: dist_bt.block_length is
        // only decremented for EXPLICIT distance codes (when we actually
        // read from the distance Huffman tree). Implicit distances (rep
        // codes, distance_code >= 0) don't touch the distance block
        // length.
        let distance: u32 = if v.distance_code >= 0 {
            // Implicit distance (kCmdLut.distance_code == 0):
            // use most recent from ring buffer. Matches upstream
            // CommandPostDecodeLiterals: --idx, dist_rb[idx&3].
            *dist_rb_idx -= 1;
            dist_rb[(*dist_rb_idx & 3) as usize]
        } else {
            // Distance block-switch (BEFORE reading the distance code).
            if dist_bt.num_block_types > 1 {
                if dist_bt.block_length == 0 {
                    dist_block_type = dist_bt.decode_switch(br)? as usize;
                    if std::env::var("BROTLI_SWITCH_LOG").is_ok() {
                        eprintln!(
                            "SW-DIST pos={} type={} len={}",
                            output_base + output.len(),
                            dist_block_type,
                            dist_bt.block_length
                        );
                    }
                }
                dist_bt.block_length -= 1;
            }

            // Read distance code from distance tree.
            // distance_context = v.context (per upstream ReadCommandInternal).
            let dist_context = v.context as usize;
            let dist_tree_idx = dist_context_map
                [(dist_block_type << K_DISTANCE_CONTEXT_BITS) + dist_context]
                as usize;
            let dist_tree = &dist_trees[dist_tree_idx];
            let dist_code = dist_tree.read_symbol(br).ok_or("invalid distance symbol")? as i32;
            if std::env::var("BROTLI_DIST_TRACE").is_ok() {
                eprintln!(
                    "DECDIST n={dec_cmd_n} sym={dist_code} ctx={dist_context} tree={dist_tree_idx} bit={}",
                    br.bit_pos()
                );
            }
            if dbg_dc_flag {
                eprintln!(
                    "DCREAD pos={} ctx={} tree={} sym={} rb={:?}",
                    output_base + output.len(),
                    dist_context,
                    dist_tree_idx,
                    dist_code,
                    dist_rb
                );
            }
            dist_sym = dist_code;
            if stats_flag {
                let mut st = dec_stats();
                *st.dist_code_hist.entry(dist_code).or_insert(0u64) += 1;
                *st.dist_hists
                    .entry(dist_tree_idx)
                    .or_default()
                    .entry(dist_code.unsigned_abs())
                    .or_insert(0) += 1;
            }
            crate::decoder::decode_distance_from_code(
                dist_code,
                num_direct_distance_codes,
                npostfix as i32,
                br,
                &mut *dist_rb,
                &mut *dist_rb_idx,
            )
        };

        // Per upstream: max_distance = min(pos, max_backward_distance).
        // pos is the CUMULATIVE output position across all metablocks.
        let pos = (output_base + output.len()) as u32;
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
                if std::env::var("BROTLI_DICT_DEBUG").is_ok() {
                    eprintln!(
                        "DICT-FAIL copy_len={copy_len} distance={distance} max_distance={max_distance} pos={} wbits_max={max_backward_distance}",
                        output_base + output.len()
                    );
                }
                return Err("static dictionary not supported");
            }
            // Compensate dist_rb_idx for implicit distance path (which
            // decremented idx; the dictionary path doesn't write back).
            if v.distance_code >= 0 {
                *dist_rb_idx = (*dist_rb_idx).wrapping_add(1);
            }
        } else {
            if distance == 0 || distance as usize > prior_output.len() + output.len() {
                return Err("invalid back-reference distance");
            }
            if distance as usize <= output.len() {
                let src = output.len() - distance as usize;
                lz77_copy(&mut output, src, copy_len);
            } else {
                // Cross-metablock back-reference (upstream keeps one
                // ring buffer across the frame).
                let back = distance as usize - output.len();
                if back > prior_output.len() {
                    return Err("invalid back-reference distance");
                }
                let src = prior_output.len() - back;
                let from_prior = (prior_output.len() - src).min(copy_len);
                output.extend_from_slice(&prior_output[src..src + from_prior]);
                if copy_len > from_prior {
                    // The spill reads the current metablock's output
                    // from its FIRST byte (stream position output_base),
                    // replicating onto itself as it grows — not from
                    // `output.len() - from_prior`, which is a different
                    // position whenever this metablock already emitted
                    // bytes before the copy.
                    lz77_copy(&mut output, 0, copy_len - from_prior);
                }
            }
            // Update recent-distances cache (upstream LZ77 copy path).
            dist_rb[(*dist_rb_idx & 3) as usize] = distance;
            *dist_rb_idx = (*dist_rb_idx).wrapping_add(1);
        }

        let cmd_pos = output_base + cmd_start_outlen + insert_len;
        if trace_flag {
            let mut st = dec_stats();
            if (st.at == 1 || (st.at != u64::MAX && (cmd_pos as u64).abs_diff(st.at) <= 512))
                && copy_len > 0
            {
                st.trace.push((
                    cmd_pos,
                    insert_len as u32,
                    copy_len as u32,
                    dist_sym,
                    distance,
                    v.distance_code >= 0,
                    *dist_rb,
                    *dist_rb_idx,
                ));
            }
        }
        // Diagnostic: dump command-stream statistics (env-gated).
        if stats_flag {
            let mut st = dec_stats();
            st.cmds += 1;
            st.literal_count += insert_len as u64;
            st.ins_extra += u64::from(v.insert_len_extra_bits);
            st.cpy_extra += u64::from(v.copy_len_extra_bits);
            st.cmd_hists.entry(cmd_block_type).or_insert([0u32; 704])[cmd_code] += 1;
            if cmd_dump_on() && copy_len > 0 {
                st.cmd_dump
                    .push((cmd_pos, insert_len as u32, copy_len as u32, distance));
            }
            if copy_len > 0 {
                st.copies += 1;
                st.copy_bytes += copy_len as u64;
                if copy_len as u64 > st.max_copy {
                    st.max_copy = copy_len as u64;
                }
                if v.distance_code >= 0 {
                    st.implicit += 1;
                }
                let bucket = match distance {
                    0..=4 => "d1-4",
                    5..=16 => "d5-16",
                    17..=64 => "d17-64",
                    65..=256 => "d65-256",
                    257..=1024 => "d257-1k",
                    1025..=8192 => "d1k-8k",
                    8193..=65536 => "d8k-64k",
                    _ => "dict",
                };
                *st.dist_hist.entry(bucket).or_insert(0u64) += 1;
                st.last_dists.push_back(distance);
                if st.last_dists.len() > 24 {
                    st.last_dists.pop_front();
                }
                st.recent_cmds
                    .push_back((insert_len as u32, copy_len as u32, distance));
                if st.recent_cmds.len() > 4000 {
                    st.recent_cmds.pop_front();
                }
            }
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
    if t_dbg {
        let t_cmd = t0.elapsed() - t_trees;
        eprintln!(
            "MBTIMER base={output_base} mlen={mlen} trees={t_trees:?} cmdloop={t_cmd:?} ntreesl={ntreesl} ncmd={} ndist={ntreesd}",
            cmd_bt.num_block_types
        );
    }

    Ok((br.bit_pos(), output, (p1, p2)))
}
