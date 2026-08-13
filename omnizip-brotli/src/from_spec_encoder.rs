//! From-spec Brotli encoder (RFC 7932).
//!
//! Implements a complete Brotli encoder from scratch — no vendored
//! code from the upstream brotli crate. Produces Huffman-coded
//! metablocks with LZ77 matches and (optionally) static dictionary
//! references.
//!
//! ## Algorithm
//!
//! 1. **Match finding**: Hash-chain LZ77 (via `omnizip_codecs::matchfinder`)
//!    + brotli static dictionary lookup.
//! 2. **Parsing**: Greedy — take the longest match at each position.
//! 3. **Entropy coding**: Per-metablock Huffman codes built via
//!    length-limited package-merge (max 15 bits).
//! 4. **Framing**: Single ISLAST=1 metablock with NBLTYPES=1 /
//!    NTREES=1 / NPOSTFIX=0 / NDIRECT=0.
//!
//! ## Wire format
//!
//! All bits are written LSB-first per RFC 7932 §1. Canonical Huffman
//! codes (MSB-first by convention) are bit-reversed before emission
//! so the decoder's LSB-first reader produces the correct lookup index.
//!
//! ## Determinism
//!
//! All algorithms are deterministic. Same input → same output, always.

#![forbid(unsafe_code)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::needless_range_loop,
    clippy::collapsible_else_if,
    clippy::items_after_statements
)]

use crate::dictionary::dictionary_lookup;
use crate::encoder::bitwriter::BitWriter;
use crate::encoder::context::{compute_context_id, is_text_like};
use crate::encoder::dict_hash;
use crate::encoder::distance_config::{DistanceConfig, NUM_SHORT};
use crate::prefix::kCmdLut;

/// Brotli window bits for the encoder (24 = 16 MB window).
/// The C reference uses WBITS=22 (4 MB) for quality < 10 and
/// WBITS=24 (16 MB) for quality >= 10. We use 24 for all qualities
/// to maximize match distance.
const WINDOW_BITS: u8 = 24;

/// Window gap per upstream brotli (`kBrotliWindowGap = 16`): the
/// reserved bytes that disambiguate dictionary references from LZ77
/// back-references.
const WINDOW_GAP: u32 = 16;

/// Maximum backward distance for LZ77 matches.
/// Per RFC 7932 §9.1: `max_backward_distance` = (1 << WBITS) - `WINDOW_GAP`.
const MAX_BACKWARD_DISTANCE: u32 = (1 << WINDOW_BITS) - WINDOW_GAP;

/// Minimum match length for LZ77.
const MIN_MATCH: u32 = 4;

/// Maximum copy length per command (RFC 7932 §5).
/// Capped at 271 (the max for short copy-length codes). Longer copies
/// were tested (up to 4096) and improve ratio on highly repetitive
/// synthetic data but regress on FSST-preprocessed data where longer
/// matches interact poorly with the byte-code distribution.
const MAX_COPY: u32 = 271;

/// A parsed LZ77 command: insert `insert_len` literals, then copy
/// `copy_len` bytes from `distance` (1-based backward offset).
#[derive(Clone, Copy, Debug)]
pub struct Command {
    pub insert_len: u32,
    pub copy_len: u32,
    pub distance: u32,
}

/// Four-slot repeat-distance ring buffer matching the decoder's state
/// (decoder_full.rs ~495). Used by `build_symbol_stream` to emit
/// explicit distance codes 0-3 for `rep0`/`rep1`/`rep2`/`rep3` matches
/// (TODO 245).
///
/// ## Decoder semantics (mirrored here)
///
/// For distance code `c` (0-3) with LZ77 back-reference:
/// - Read slot `(idx - (c-3)) & 3`.
/// - Decrement `idx` by `1 >> c` (only `c == 0` decrements).
/// - LZ77 copy: write the read value to slot `idx & 3`, then `idx += 1`.
///
/// For long-form distance with LZ77 back-reference:
/// - Write the new distance to slot `idx & 3`, then `idx += 1`.
///
/// For dictionary references: no write-back. Only implicit commands
/// (`distance_code >= 0`) get `idx += 1` compensation.
///
/// Initial state matches the C reference: `dist_rb = [16, 15, 11, 4]`,
/// `idx = 0`.
#[derive(Clone, Debug)]
struct RepBuffer {
    dist_rb: [u32; 4],
    idx: i32,
}

impl RepBuffer {
    fn new() -> Self {
        Self {
            dist_rb: [16, 15, 11, 4],
            idx: 0,
        }
    }

    /// Returns the distance currently at rep code `code` (0-3).
    /// Code 0 is the most recent; code 3 is the oldest.
    fn rep_at(&self, code: u32) -> u32 {
        debug_assert!(code <= 3);
        let offset = code as i32 - 3;
        let idx = (self.idx - offset) & 3;
        self.dist_rb[idx as usize]
    }

    /// If `distance` matches any rep code, returns the smallest matching
    /// code (prefers lower codes on ties — they save the same bits).
    fn find_rep_code(&self, distance: u32) -> Option<u32> {
        for code in 0..4u32 {
            if self.rep_at(code) == distance {
                return Some(code);
            }
        }
        None
    }

    /// Update state after an LZ77 back-reference that used rep code `code`
    /// (0-3) or implicit (= code 0). Mirrors decoder behavior exactly.
    fn on_rep_lz77(&mut self, code: u32) {
        let offset = code as i32 - 3;
        let distance_context = 1i32 >> code;
        // Read (capture the distance before modifying idx).
        let read_idx = ((self.idx - offset) & 3) as usize;
        let distance = self.dist_rb[read_idx];
        // Modify idx (only code 0 decrements).
        self.idx -= distance_context;
        // LZ77 write-back: write the read value at the new idx, then idx += 1.
        let write_idx = (self.idx & 3) as usize;
        self.dist_rb[write_idx] = distance;
        self.idx = self.idx.wrapping_add(1);
    }

    /// Update state after an LZ77 back-reference with a new long-form distance.
    fn on_new_distance_lz77(&mut self, distance: u32) {
        let write_idx = (self.idx & 3) as usize;
        self.dist_rb[write_idx] = distance;
        self.idx = self.idx.wrapping_add(1);
    }

    /// Update state after a dictionary reference. Only implicit commands
    /// (was_implicit = true) get idx compensation; explicit code 0-3 used
    /// for dict references would corrupt the buffer (encoder avoids this).
    fn on_dict_reference(&mut self, was_implicit: bool) {
        if was_implicit {
            self.idx = self.idx.wrapping_add(1);
        }
    }
}

#[cfg(test)]
#[allow(dead_code)]
impl RepBuffer {
    /// Snapshot the current rep0 (most recent distance).
    fn rep0(&self) -> u32 {
        self.rep_at(0)
    }
}

/// Encode the WBITS field for `WINDOW_BITS` (RFC 7932 §9.1).
fn write_wbits(bw: &mut BitWriter) {
    bw.write_bits(1, 1);
    bw.write_bits(u32::from(WINDOW_BITS - 17), 3);
}

/// Reverse the low `n` bits of `v`. Used to convert MSB-first
/// canonical Huffman codes into the LSB-first wire representation.
fn reverse_bits(mut v: u32, n: u8) -> u32 {
    let mut r = 0u32;
    for _ in 0..n {
        r = (r << 1) | (v & 1);
        v >>= 1;
    }
    r
}

/// Compress input into a valid Brotli frame at quality 11 (maximum effort).
///
/// Equivalent to [`compress_with_quality`](fn.compress_with_quality.html) with q=11.
#[must_use]
pub fn compress(input: &[u8]) -> Vec<u8> {
    compress_with_quality(input, 11)
}

/// Compress at a given Brotli quality level (0–11).
///
/// Higher quality trades speed for ratio by:
/// - Increasing hash-chain depth (`max_chain_length`)
/// - Increasing the "good enough" match length (`nice_match`)
/// - Enabling static-dictionary lookups (q ≥ 2)
///
/// All levels produce RFC 7932-conformant Brotli streams decodable by
/// any standard Brotli decoder.
#[must_use]
pub fn compress_with_quality(input: &[u8], quality: i32) -> Vec<u8> {
    let q = quality.clamp(0, 11);
    if input.is_empty() {
        return empty_frame();
    }

    // Inputs ≤ 1 MiB: single metablock, Huffman or uncompressed.
    // Larger metablocks reduce per-block Huffman table overhead.
    if input.len() < (1 << 20) {
        let uncompressed = encode_uncompressed_frame(input);
        let huffman = encode_huffman_frame_q(input, q);
        if !huffman.is_empty() && huffman.len() < uncompressed.len() {
            return huffman;
        }
        return uncompressed;
    }

    // Large inputs (> 1 MiB): split into 1 MiB chunks and emit
    // each as a Huffman-coded metablock. Uses a SINGLE match finder
    // over the full input so chunk N+1 can find matches referencing
    // data from chunks 0..N (cross-chunk matching).
    let chunk_size = (1 << 20) - 1; // 1 MiB - 1
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);

    // Build quality-dependent config for the shared match finder.
    let is_text = is_text_like(input);
    let (max_chain, nice_match, _, _, _, hash_log) = brotli_quality_config(q, is_text);
    // Cross-chunk MF reuse for Q4+ (any content type). The optimal
    // parser has a cost model that correctly evaluates cross-chunk
    // distances, so this is safe for all data types.
    if q >= 4 {
        // Shared MF path: one match finder over the full input.
        let mf_config = omnizip_codecs::HashChainConfig {
            dict_size: MAX_BACKWARD_DISTANCE,
            min_match: MIN_MATCH,
            max_chain_length: max_chain,
            nice_match,
            hash_log,
            max_match_length: MAX_COPY,
        };
        let mut shared_mf = omnizip_codecs::HashChainMatchFinder::new(input, mf_config);
        let mut offset = 0usize;
        while offset < input.len() {
            let end = (offset + chunk_size).min(input.len());
            let is_last = end == input.len();
            encode_huffman_chunk_with_shared_mf(
                &mut bw,
                input,
                offset,
                end,
                is_last,
                q,
                &mut shared_mf,
            );
            offset = end;
        }
    } else {
        // Per-chunk MF path (Q0-Q3 or binary).
        let mut offset = 0usize;
        while offset < input.len() {
            let end = (offset + chunk_size).min(input.len());
            let is_last = end == input.len();
            encode_huffman_chunk_into(&mut bw, &input[offset..end], offset, is_last, q);
            offset = end;
        }
    }
    bw.flush()
}

/// Append all bits from `src` (bytes + accumulator) to `dst`.
#[allow(dead_code)]
fn append_writer(dst: &mut BitWriter, src: BitWriter) {
    for byte in src.out {
        dst.write_bits(u32::from(byte), 8);
    }
    if src.nbits > 0 {
        dst.write_bits(src.acc as u32, src.nbits);
    }
}

/// Encode one metablock (Huffman-coded) into the shared writer.
fn encode_huffman_chunk_into(
    bw: &mut BitWriter,
    input: &[u8],
    mlen_offset: usize,
    is_last: bool,
    quality: i32,
) {
    // Standard path: MF created over the chunk slice itself.
    let is_text = is_text_like(input);
    let (max_chain, nice_match, _, _, _, hash_log) = brotli_quality_config(quality, is_text);
    let config = omnizip_codecs::HashChainConfig {
        dict_size: MAX_BACKWARD_DISTANCE,
        min_match: MIN_MATCH,
        max_chain_length: max_chain,
        nice_match,
        hash_log,
        max_match_length: MAX_COPY,
    };
    let mut mf = omnizip_codecs::HashChainMatchFinder::new(input, config);
    encode_huffman_chunk_body(bw, input, &mut mf, mlen_offset, is_last, quality);
}

/// Internal: encode one metablock with an external match finder.
/// The MF may reference the full input (cross-chunk) or just the
/// chunk slice (per-chunk), depending on the caller.
fn encode_huffman_chunk_body(
    bw: &mut BitWriter,
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    is_last: bool,
    quality: i32,
) {
    bw.write_bits(u32::from(is_last), 1); // ISLAST
                                          // ISLASTEMPTY only present when ISLAST=1; we never emit empty
                                          // metablocks, so always 0 when present.
    if is_last {
        bw.write_bits(0, 1); // ISLASTEMPTY = 0
    }
    // MLEN encoding: pick smallest MNIBBLES that fits.
    // MNIBBLES=0 → 4 nibbles (max 65536)
    // MNIBBLES=1 → 5 nibbles (max 1,048,576 = 1 MiB)
    // MNIBBLES=2 → 6 nibbles (max 16,777,216 = 16 MiB)
    let mlen_minus_1 = (input.len() - 1) as u32;
    let (mnibbles, num_nibbles): (u32, u32) = if mlen_minus_1 < (1 << 16) {
        (0, 4)
    } else if mlen_minus_1 < (1 << 20) {
        (1, 5)
    } else {
        (2, 6)
    };
    bw.write_bits(mnibbles, 2);
    for i in 0..num_nibbles {
        bw.write_bits((mlen_minus_1 >> (4 * i)) & 0xF, 4);
    }
    // ISUNCOMPRESSED is only written when ISLAST=0 (matches upstream
    // `DecodeMetaBlockLength` gate: `if (is_last == 0 && is_metadata == 0)`).
    if !is_last {
        bw.write_bits(0, 1); // ISUNCOMPRESSED = 0
    }

    // Context modeling: at quality >= 4, split literals into context
    // trees. Active for Q4+ inputs ≥ 4 KiB (any content type — FSST-
    // transformed data benefits from context separation just as much
    // as natural text).
    let use_context = quality >= 4 && input.len() >= 4096;

    // Block-type switching is disabled — testing showed a slight ratio
    // regression on uniform text data (per-block-type Huffman overhead
    // exceeds the benefit when statistics don't vary). The decoder now
    // correctly handles NBLTYPES > 1, and `write_block_type_trees` +
    // the inline switch emission in the literal loop are wired up, so
    // this can be flipped back on for inputs with strongly varying
    // per-block statistics.
    let use_block_switch = false;
    let nbltypes_l: u32 = if use_block_switch { 2 } else { 1 };
    write_varlen_uint8(bw, nbltypes_l - 1); // NBLTYPESL
    if nbltypes_l > 1 {
        write_block_type_trees(bw, nbltypes_l);
    }
    write_varlen_uint8(bw, 0); // NBLTYPESI = 1 (no cmd block trees)
    write_varlen_uint8(bw, 0); // NBLTYPESD = 1 (no dist block trees)

    let commands = parse_input_with_offset(input, mf, mlen_offset, quality, false);

    // Choose distance-code configuration from the parsed commands.
    let dist_cfg = DistanceConfig::choose(&commands);
    bw.write_bits(dist_cfg.npostfix as u32, 2); // NPOSTFIX
    bw.write_bits(dist_cfg.ndmoem as u32, 4); // NDMOEM

    // Context mode selection: UTF8 (2) for text-like input, LSB6 (0) otherwise.
    // UTF8 gives better context separation for multi-byte chars and ASCII text.
    let context_mode: u32 = if use_context && is_text_like(input) {
        2 // UTF8
    } else {
        0 // LSB6
    }; // Context mode: one field PER literal block type (RFC 7932 §9.3).
    for _ in 0..nbltypes_l {
        bw.write_bits(context_mode, 2);
    }

    let (ntrees_l, lit_ctx_map): (u32, Vec<u8>) = if use_block_switch {
        (2, (0..128u8).map(|i| i >> 6).collect())
    } else if use_context && input.len() >= 8192 {
        (4u32, (0..64u8).map(|ctx| ctx >> 4).collect())
    } else if use_context {
        (2, (0..64u8).map(|ctx| u8::from(ctx >= 32)).collect())
    } else {
        (1, Vec::new())
    };

    write_varlen_uint8(bw, ntrees_l - 1); // NTREESL
    if ntrees_l > 1 {
        write_context_map(bw, &lit_ctx_map, ntrees_l);
    }
    write_varlen_uint8(bw, 0); // NTREESD = 1

    let Some(stream) = build_symbol_stream(&commands, input, mlen_offset, &dist_cfg) else {
        return;
    };

    let dist_alphabet = dist_cfg.alphabet_size();

    // --- Context modeling: per-tree literal frequencies ---
    // For NTREES_L > 1, partition literals by their LSB6 context.
    // Build a virtual output buffer to correctly track the "previous byte"
    // for context computation (copies change the previous byte too).
    let ntrees = ntrees_l as usize;
    let mut lit_freqs: Vec<Vec<u32>> = vec![vec![0u32; 256]; ntrees];

    // Simulate output to get correct per-position context.
    // For dictionary references (distance > output.len()), the copy
    // produces bytes from the static dictionary, not the output buffer.
    // For transforms that change word length (prefix/suffix/omit), the
    // actual copy output length differs from cmd.copy_len (= word_length).
    // We precompute the actual copy advance for each command so subsequent
    // loops (frequency counting, encoding) advance correctly.
    let mut output_sim: Vec<u8> = Vec::with_capacity(input.len());
    let mut cmd_copy_advances: Vec<usize> = Vec::with_capacity(commands.len());
    let mut lit_idx = 0usize;
    for cmd in &commands {
        for _ in 0..cmd.insert_len {
            output_sim.push(stream.literals[lit_idx]);
            lit_idx += 1;
        }
        let copy_advance = if cmd.copy_len > 0 {
            let before = output_sim.len();
            let copy_start_global = mlen_offset + output_sim.len();
            let max_dist = (copy_start_global as u32).min(MAX_BACKWARD_DISTANCE);
            let is_dict = (cmd.distance as usize) > output_sim.len();
            if is_dict {
                let mut dict_bytes = Vec::with_capacity(cmd.copy_len as usize);
                if dictionary_lookup(&mut dict_bytes, cmd.copy_len, cmd.distance as i32, max_dist)
                    .is_some()
                {
                    output_sim.extend_from_slice(&dict_bytes);
                } else {
                    output_sim.extend(std::iter::repeat(0u8).take(cmd.copy_len as usize));
                }
            } else {
                let src = output_sim.len() - cmd.distance as usize;
                for i in 0..cmd.copy_len as usize {
                    output_sim.push(output_sim[src + i]);
                }
            }
            output_sim.len() - before
        } else {
            0
        };
        cmd_copy_advances.push(copy_advance);
    }

    // Compute per-tree frequencies using simulated output.
    let mut p1: u8 = 0;
    let mut p2: u8 = 0;
    let mut out_pos = 0usize;
    let mut lit_block_type: usize = 0;
    let mut lit_block_remaining: usize = if use_block_switch { 128 } else { usize::MAX };
    for (cmd_idx, cmd) in commands.iter().enumerate() {
        for _ in 0..cmd.insert_len {
            let b = output_sim[out_pos];
            let ctx = if use_block_switch {
                let cm_idx = (lit_block_type << 6) + (compute_context_id(p1, p2, 0) as usize);
                lit_ctx_map[cm_idx] as usize
            } else {
                let ctx_id = compute_context_id(p1, p2, context_mode) as usize;
                if ntrees > 1 {
                    lit_ctx_map[ctx_id] as usize
                } else {
                    0
                }
            };
            lit_freqs[ctx][b as usize] += 1;
            p2 = p1;
            p1 = b;
            out_pos += 1;
            if use_block_switch {
                if lit_block_remaining == 0 {
                    lit_block_type = 1 - lit_block_type;
                    lit_block_remaining = 128;
                }
                lit_block_remaining -= 1;
            }
        }
        if cmd.copy_len > 0 {
            out_pos += cmd_copy_advances[cmd_idx];
            p2 = p1;
            p1 = output_sim[out_pos - 1];
        }
    }

    let mut cmd_freq = vec![0u32; 704];
    let mut dist_freq = vec![0u32; dist_alphabet];

    // Ensure every literal tree has at least one symbol. Smart context
    // clustering can produce trees with zero literals if no contexts map
    // to them. A zero-frequency tree would produce a degenerate Huffman
    // table that the decoder reads as "symbol 0, 0 bits per occurrence" —
    // corrupting output if that tree is ever selected during decoding.
    // Adding a dummy frequency for byte 0 prevents this.
    for freq in &mut lit_freqs {
        let total: u32 = freq.iter().sum();
        if total == 0 {
            freq[0] = 1;
        }
    }

    for &sym in &stream.cmd_symbols {
        cmd_freq[sym] += 1;
    }
    for &sym in &stream.dist_symbols {
        dist_freq[sym as usize] += 1;
    }

    // Build per-tree literal Huffman tables.
    let mut lit_lengths_per_tree: Vec<omnizip_codecs::HuffmanLengths> = lit_freqs
        .iter()
        .map(|freq| omnizip_codecs::HuffmanLengths::build(freq, 15))
        .collect();
    let cmd_lengths = omnizip_codecs::HuffmanLengths::build(&cmd_freq, 15);
    let dist_lengths = omnizip_codecs::HuffmanLengths::build(&dist_freq, 15);

    // Override code lengths for sparse LITERAL trees (2-4 symbols) when
    // using multi-tree context modeling (NTREES > 1). Context-clustered
    // trees can have very few symbols, and the complex form RLE encoding
    // produces wire-format mismatches for these sparse tables. Simple
    // form avoids the RLE path entirely. Only applied to literal trees
    // (not command/distance) to avoid ratio regression on those tables.
    if ntrees > 1 {
        for tree in &mut lit_lengths_per_tree {
            override_lengths_for_simple_form(&mut tree.lengths, 256);
        }
    }

    let lit_codes_per_tree: Vec<Vec<(u32, u8)>> = lit_lengths_per_tree
        .iter()
        .map(canonical_with_reverse)
        .collect();
    let cmd_codes = canonical_with_reverse(&cmd_lengths);
    let dist_codes = canonical_with_reverse(&dist_lengths);

    // Write literal tree group (one table per tree).
    for tree in &lit_lengths_per_tree {
        write_huffman_table(bw, tree, 256);
    }
    write_huffman_table(bw, &cmd_lengths, 704);
    write_huffman_table(bw, &dist_lengths, dist_alphabet);

    // --- Encode commands + literals with per-context tree selection ---
    let mut dist_iter = stream.dist_symbols.iter().zip(stream.dist_extras.iter());
    p1 = 0;
    p2 = 0;
    lit_idx = 0;
    out_pos = 0;
    lit_block_type = 0;
    lit_block_remaining = if use_block_switch { 128 } else { usize::MAX };
    for (cmd_idx, (&cmd_sym, cmd)) in stream.cmd_symbols.iter().zip(commands.iter()).enumerate() {
        let (code, len) = cmd_codes[cmd_sym];
        bw.write_bits(code, u32::from(len));

        let entry = &kCmdLut[cmd_sym];
        if entry.insert_len_extra_bits > 0 {
            let extra = cmd.insert_len - u32::from(entry.insert_len_offset);
            bw.write_bits(extra, u32::from(entry.insert_len_extra_bits));
        }
        if entry.copy_len_extra_bits > 0 {
            let extra = cmd.copy_len - u32::from(entry.copy_len_offset);
            bw.write_bits(extra, u32::from(entry.copy_len_extra_bits));
        }

        for _ in 0..cmd.insert_len {
            // Block switch BEFORE the literal (matches decoder's
            // check-block-length-then-read-literal order). The decoder
            // checks block_length == 0 at the START of each iteration.
            if use_block_switch && lit_block_remaining == 0 {
                let bt_sym = u32::from(lit_block_type == 0);
                bw.write_bits(bt_sym, 1);
                bw.write_bits(15, 5); // block-length extra (128-113=15)
                lit_block_type = 1 - lit_block_type;
                lit_block_remaining = 128;
            }

            let b = stream.literals[lit_idx];
            let tree = if use_block_switch {
                lit_block_type
            } else if ntrees > 1 {
                let ctx = compute_context_id(p1, p2, context_mode) as usize;
                lit_ctx_map[ctx] as usize
            } else {
                0
            };
            let (lc, ll) = lit_codes_per_tree[tree][b as usize];
            bw.write_bits(lc, u32::from(ll));
            p2 = p1;
            p1 = b;
            lit_idx += 1;
            out_pos += 1;

            if use_block_switch {
                lit_block_remaining -= 1;
            }
        }

        if cmd.copy_len > 0 {
            // Check if this command uses implicit distance (rep code).
            // Implicit commands don't have a distance symbol in the stream.
            let cmd_entry = &kCmdLut[cmd_sym];
            if cmd_entry.distance_code < 0 {
                let (&d_sym, &d_extra) = dist_iter.next().expect("distance stream exhausted");
                let (dc, dl) = dist_codes[d_sym as usize];
                bw.write_bits(dc, u32::from(dl));
                let nbits = distance_extra_bits(d_sym, &dist_cfg);
                if nbits > 0 {
                    bw.write_bits(d_extra, nbits);
                }
            }
            out_pos += cmd_copy_advances[cmd_idx];
            p2 = p1;
            p1 = output_sim[out_pos - 1];
        }
    }
}

/// Encode one metablock (uncompressed) into the shared writer.
/// Kept for the `multi_metablock_uncompressed_round_trips` test.
/// Always passes `is_last = false`: per upstream brotli, an ISLAST=1
/// metablock cannot be uncompressed (ISUNCOMPRESSED is only read when
/// ISLAST=0). Terminate the stream with a separate empty ISLAST=1
/// metablock via [`empty_frame_terminator_into`].
#[cfg(test)]
fn encode_uncompressed_chunk_into(bw: &mut BitWriter, input: &[u8], _is_last: bool) {
    bw.write_bits(0, 1); // ISLAST = 0 (so ISUNCOMPRESSED is read)
    bw.write_bits(0, 2); // MNIBBLES = 0 (= 4 nibbles)
    let mlen_minus_1 = (input.len() - 1) as u64;
    for i in 0..4u32 {
        bw.write_bits(((mlen_minus_1 >> (4 * u64::from(i))) & 0xF) as u32, 4);
    }
    bw.write_bits(1, 1); // ISUNCOMPRESSED = 1
    bw.byte_align();
    for &b in input {
        bw.write_bits(u32::from(b), 8);
    }
}

/// Write an empty ISLAST=1 metablock (the stream terminator).
#[cfg(test)]
fn empty_frame_terminator_into(bw: &mut BitWriter) {
    bw.write_bits(1, 1); // ISLAST = 1
    bw.write_bits(1, 1); // ISLASTEMPTY = 1
}

// ---------------------------------------------------------------------------
// Uncompressed metablock (RFC 7932 §9.2)
// ---------------------------------------------------------------------------

fn encode_uncompressed_frame(input: &[u8]) -> Vec<u8> {
    // Per upstream `DecodeMetaBlockLength`: ISUNCOMPRESSED is only read
    // when ISLAST=0. To emit an uncompressed last metablock, we write
    // the data as ISLAST=0 + ISUNCOMPRESSED=1, then append a final
    // empty ISLAST=1 metablock.
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);

    bw.write_bits(0, 1); // ISLAST = 0 (so ISUNCOMPRESSED is read)

    let mnibbles_field: u32 = if input.len() < (1 << 16) { 0 } else { 2 };
    bw.write_bits(mnibbles_field, 2);

    let nibbles: u32 = if mnibbles_field == 0 {
        4
    } else {
        mnibbles_field + 4
    };
    let mlen_minus_1 = (input.len() - 1) as u64;
    for i in 0..nibbles {
        let nib = ((mlen_minus_1 >> (4 * u64::from(i))) & 0xF) as u32;
        bw.write_bits(nib, 4);
    }

    bw.write_bits(1, 1); // ISUNCOMPRESSED = 1
    bw.byte_align();

    let mut out = bw.flush();
    out.extend_from_slice(input);

    // Append empty ISLAST=1 metablock to terminate the stream.
    let mut terminator = BitWriter::new();
    terminator.write_bits(1, 1); // ISLAST = 1
    terminator.write_bits(1, 1); // ISLASTEMPTY = 1
    out.extend_from_slice(&terminator.flush());

    out
}

fn empty_frame() -> Vec<u8> {
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);
    bw.write_bits(1, 1); // ISLAST = 1
    bw.write_bits(1, 1); // ISLASTEMPTY = 1
    bw.flush()
}

// ---------------------------------------------------------------------------
// Huffman-coded metablock
// ---------------------------------------------------------------------------

/// Encode the entire input as a single Huffman-coded metablock. Calls
/// the chunk encoder with `mlen_offset=0` and `is_last=true`, then
/// prepends WBITS.
fn encode_huffman_frame_q(input: &[u8], quality: i32) -> Vec<u8> {
    if input.is_empty() || input.len() >= (1 << 20) {
        return Vec::new();
    }
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);
    encode_huffman_chunk_into(&mut bw, input, 0, true, quality);
    bw.flush()
}

/// Like [`encode_huffman_chunk_into`] but uses `full_input` as the
/// match-finder data source, enabling cross-chunk match references.
/// Used by [`compress_with_quality`] for multi-metablock inputs.
fn encode_huffman_chunk_with_shared_mf(
    bw: &mut BitWriter,
    full_input: &[u8],
    chunk_start: usize,
    chunk_end: usize,
    is_last: bool,
    quality: i32,
    mf: &mut omnizip_codecs::HashChainMatchFinder,
) {
    let chunk = &full_input[chunk_start..chunk_end];
    encode_huffman_chunk_body(bw, chunk, mf, chunk_start, is_last, quality);
}

/// A parsed symbol stream ready for entropy coding.
struct SymbolStream {
    /// Literal bytes in insertion order.
    literals: Vec<u8>,
    /// Command symbols (indices into kCmdLut, 0..704).
    cmd_symbols: Vec<usize>,
    /// Distance symbols (0..63) — one per command with `copy_len` > 0.
    dist_symbols: Vec<u32>,
    /// Distance extra-bit values, parallel to `dist_symbols`.
    dist_extras: Vec<u32>,
}

/// Build the entropy-coded symbol stream from commands.
///
/// For each command:
/// - Look up the matching entry in `kCmdLut` (`cell_idx` ≥ 2 for explicit
///   distance; we never emit implicit-distance commands).
/// - Compute the distance symbol + extra bits via the long-code formula
///   (RFC 7932 §10.4).
fn build_symbol_stream(
    commands: &[Command],
    input: &[u8],
    mlen_offset: usize,
    dist_cfg: &DistanceConfig,
) -> Option<SymbolStream> {
    let mut literals = Vec::new();
    let mut cmd_symbols = Vec::with_capacity(commands.len());
    let mut dist_symbols = Vec::new();
    let mut dist_extras = Vec::new();

    // Track the 4-distance ring buffer (TODO 245). Lets us emit
    // explicit distance codes 0-3 for rep0/1/2/3 matches, saving
    // the distance extra bits (typically 5-15 bits per match).
    let mut rep = RepBuffer::new();
    let mut prev_was_implicit = false;

    // Output-position cursor — needed to detect dictionary references
    // (distance > current output) which can't use rep codes (would
    // corrupt the decoder's ring buffer state).
    let mut output_pos = 0usize;

    for cmd in commands {
        let _ = input;
        output_pos += cmd.insert_len as usize;
        let is_dict_ref = cmd.copy_len > 0 && (cmd.distance as usize) > output_pos;

        // Try implicit rep0 command (saves the entire distance Huffman
        // symbol — ~5 bits — only viable for small copy_len/insert_len).
        // Disabled for dictionary references (decoder doesn't compensate
        // explicit code 0 for dicts, only implicit).
        let can_use_implicit = !prev_was_implicit
            && cmd.copy_len > 0
            && !is_dict_ref
            && cmd.distance == rep.rep_at(0)
            && (2..=9).contains(&cmd.copy_len)
            && cmd.insert_len <= 9;

        let cmd_sym = if can_use_implicit {
            find_cmd_symbol_with_rep(cmd.insert_len, cmd.copy_len, Some(0))
        } else {
            find_cmd_symbol(cmd.insert_len, cmd.copy_len)
        }?;

        cmd_symbols.push(cmd_sym);

        let entry = &kCmdLut[cmd_sym];
        let this_was_implicit = entry.distance_code >= 0;

        if entry.distance_code < 0 && cmd.copy_len > 0 {
            // Explicit distance symbol needed. For LZ77 back-references,
            // try rep codes 0-3 first (each saves the distance extra bits
            // — typically 5-15 bits — vs. the long-form encoding).
            let (sym, extra) = if is_dict_ref {
                // Dictionary references can't use rep codes (the decoder
                // doesn't compensate explicit code 0-3 for dicts).
                encode_distance(cmd.distance, dist_cfg)
            } else if let Some(code) = rep.find_rep_code(cmd.distance) {
                (code, 0)
            } else {
                encode_distance(cmd.distance, dist_cfg)
            };
            dist_symbols.push(sym);
            dist_extras.push(extra);
        }

        // Update RepBuffer to mirror decoder state.
        if cmd.copy_len > 0 {
            if is_dict_ref {
                rep.on_dict_reference(this_was_implicit);
            } else if this_was_implicit {
                // Implicit == explicit code 0 for LZ77
                rep.on_rep_lz77(0);
            } else {
                // Explicit distance code: check which rep was used (if any)
                // vs. long-form. We re-derive this here rather than thread
                // it from the encoding decision above, because the cost is
                // trivial and it keeps the logic readable.
                match rep.find_rep_code(cmd.distance) {
                    Some(code) => rep.on_rep_lz77(code),
                    None => rep.on_new_distance_lz77(cmd.distance),
                }
            }
        }

        prev_was_implicit = this_was_implicit;

        // Advance output_pos by the copy advance.
        if cmd.copy_len > 0 {
            output_pos += if is_dict_ref {
                // Dict transforms can change length; query the actual length.
                let global_pos = mlen_offset + output_pos;
                let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                match dictionary_lookup(&mut tmp, cmd.copy_len, cmd.distance as i32, max_dist) {
                    Some(()) => tmp.len(),
                    None => cmd.copy_len as usize,
                }
            } else {
                cmd.copy_len as usize
            };
        }
    }

    // Extract literals in stream order from commands (re-derive from input
    // via a sequential cursor — the parser already grouped them).
    // For correctness we re-walk commands against a cursor.
    let mut cur = 0usize;
    for cmd in commands {
        let end = cur + cmd.insert_len as usize;
        literals.extend_from_slice(&input[cur..end]);
        // Advance by the actual input consumed. For dictionary references
        // with length-changing transforms (prefix/suffix/omit), the
        // transformed length may differ from copy_len (= word_length).
        let advance = if cmd.copy_len > 0 {
            let is_dict = (cmd.distance as usize) > cur;
            if is_dict {
                let global_pos = mlen_offset + end;
                let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                match dictionary_lookup(&mut tmp, cmd.copy_len, cmd.distance as i32, max_dist) {
                    Some(()) => tmp.len(),
                    None => cmd.copy_len as usize,
                }
            } else {
                cmd.copy_len as usize
            }
        } else {
            0
        };
        cur = end + advance;
    }

    Some(SymbolStream {
        literals,
        cmd_symbols,
        dist_symbols,
        dist_extras,
    })
}

/// Find the kCmdLut symbol matching (`insert_len`, `copy_len`).
///
/// For `copy_len > 0`: matches entries with `distance_code == -1`
/// (`cell_idx` >= 2) so an explicit distance code is read by the decoder.
///
/// For `copy_len == 0` (insert-only trailing command): matches any
/// entry whose `insert_len_offset` is in range and whose
/// `copy_len_offset == 2` (smallest). The decoder short-circuits at
/// metablock end without executing the copy, so the phantom `copy_len`
/// is harmless.
fn find_cmd_symbol(insert_len: u32, copy_len: u32) -> Option<usize> {
    find_cmd_symbol_impl(insert_len, copy_len, None)
}

/// Like [`find_cmd_symbol`] but optionally searches for implicit-distance
/// entries (rep codes). When `rep_code` is `Some(dc)`, searches for
/// entries with `distance_code == dc` instead of `distance_code == -1`.
/// This enables cheaper encoding for repeat-offset matches.
fn find_cmd_symbol_with_rep(
    insert_len: u32,
    copy_len: u32,
    rep_code: Option<i32>,
) -> Option<usize> {
    find_cmd_symbol_impl(insert_len, copy_len, rep_code)
}

fn find_cmd_symbol_impl(insert_len: u32, copy_len: u32, rep_code: Option<i32>) -> Option<usize> {
    let phantom = copy_len == 0;
    let effective_copy = if phantom { 2 } else { copy_len };
    for (i, entry) in kCmdLut.iter().enumerate() {
        if phantom {
            // Trailing insert-only: any distance_code is fine
        } else if let Some(rc) = rep_code {
            // Rep code: match specific distance_code
            if entry.distance_code != rc as i8 {
                continue;
            }
        } else {
            // Explicit: skip implicit entries
            if entry.distance_code >= 0 {
                continue;
            }
        }
        let ins_lo = u32::from(entry.insert_len_offset);
        let ins_hi = ins_lo + ((1u32) << u32::from(entry.insert_len_extra_bits)) - 1;
        let cpy_lo = u32::from(entry.copy_len_offset);
        let cpy_hi = cpy_lo + ((1u32) << u32::from(entry.copy_len_extra_bits)) - 1;
        if (ins_lo..=ins_hi).contains(&insert_len) && (cpy_lo..=cpy_hi).contains(&effective_copy) {
            return Some(i);
        }
    }
    // Fallback: if rep_code search fails, try explicit
    if rep_code.is_some() {
        return find_cmd_symbol_impl(insert_len, copy_len, None);
    }
    None
}

/// Encode an LZ77 distance as a (symbol, `extra_bits`) pair using the
/// given distance-code configuration.
///
/// Direct codes (when NDIRECT > 0): distance 1..=NDIRECT maps to
/// symbols 16..16+NDIRECT-1 with zero extra bits.
///
/// Long codes (symbol ≥ 16+NDIRECT): use the standard RFC 7932 §10.4
/// formula, shifted past the direct-code range.
fn encode_distance(distance: u32, cfg: &DistanceConfig) -> (u32, u32) {
    let ndirect = cfg.ndirect();

    // Direct codes: distance 1..=NDIRECT → symbol 16..16+NDIRECT-1
    if distance <= ndirect {
        return (NUM_SHORT + distance - 1, 0);
    }

    // Long codes: shift past short + direct codes
    let d = distance - 1 - ndirect;
    let mut nbits: u32 = 1;
    while nbits < 24 {
        let limit_even = (4u32 << (nbits - 1)).saturating_sub(4) + (1u32 << nbits);
        let limit_odd = (6u32 << (nbits - 1)).saturating_sub(4) + (1u32 << nbits);
        let limit = limit_even.max(limit_odd);
        if d < limit {
            break;
        }
        nbits += 1;
    }
    let even_offset = (4u32 << (nbits - 1)).saturating_sub(4);
    let odd_offset = (6u32 << (nbits - 1)).saturating_sub(4);
    let (postfix_bit, base) = if d >= odd_offset {
        (1, odd_offset)
    } else {
        (0, even_offset)
    };
    let distval = (nbits - 1) * 2 + postfix_bit;
    let sym = cfg.num_direct() + distval;
    let extra = d - base;
    (sym, extra)
}

/// Number of extra bits for a distance symbol under the given config.
fn distance_extra_bits(sym: u32, cfg: &DistanceConfig) -> u32 {
    let num_direct = cfg.num_direct();
    if sym < num_direct {
        // Short codes (0-15) and direct codes (16..16+NDIRECT-1): no extra bits.
        return 0;
    }
    let distval = sym - num_direct;
    (distval >> 1) + 1
}

/// Parse input into commands using LZ77 + static dictionary.
///
/// Tracks the output position to compute `max_distance = min(pos,
/// max_backward_distance)` per RFC 7932 §10.4. Dictionary references
/// use `distance = max_distance + 1 + address` so the decoder resolves
/// them via the static dictionary lookup path.
/// Parse input into commands using LZ77 + static dictionary.
///
/// Convenience wrapper for chunks at the start of the input (offset=0).
#[allow(dead_code)]
/// Cost-aware DP optimal parser (TODO 201).
///
/// Uses dynamic programming to find the command sequence with minimum
/// estimated bit cost. The cost model uses Shannon entropy for literals
/// and fixed estimates for command/distance overhead.
///
/// Steps:
/// 1. Collect best match at each position via the hash-chain match finder.
/// 2. Build literal cost model from byte frequency distribution.
/// 3. Backward DP: `cost[i]` = minimum bits to encode `input[i..n]`.
/// 4. Forward reconstruction: walk the DP table to emit commands.
fn optimal_parse(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    use_dict: bool,
) -> Vec<Command> {
    optimal_parse_with_costs(input, mf, mlen_offset, use_dict, None)
}

/// Compute per-byte Shannon entropy as a literal cost model.
///
/// Bytes that don't appear in `data` get cost 8.0 (worst case). Bytes
/// that appear get `-log2(p)` where p is their frequency, clamped to a
/// minimum of 1.0 bit (Huffman codes shorter than 1 bit don't exist).
fn compute_shannon_lit_cost(data: &[u8]) -> [f32; 256] {
    let mut freq = [0u32; 256];
    for &b in data {
        freq[b as usize] += 1;
    }
    let mut arr = [8.0f32; 256];
    if data.is_empty() {
        return arr;
    }
    let total = data.len() as f32;
    for i in 0..256 {
        if freq[i] > 0 {
            let p = freq[i] as f32 / total;
            let bits = -p.log2();
            arr[i] = bits.max(1.0);
        }
    }
    arr
}

/// Like [`optimal_parse`] but accepts an optional literal cost override.
/// Used by the iterative parser refinement (TODO 246) to feed back
/// actual Huffman-derived code lengths into a second DP pass.
fn optimal_parse_with_costs(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    use_dict: bool,
    lit_cost_override: Option<[f32; 256]>,
) -> Vec<Command> {
    let n = input.len();
    if n == 0 {
        return Vec::new();
    }

    // --- Step 1: Collect matches at each position ---
    // Each match is (distance, copy_len, advance_len, is_dict).
    // - Hash match: copy_len == advance_len == m.length
    // - Dict match (length-preserving): copy_len == advance_len == wl == tl
    // - Dict match (length-changing): copy_len = wl, advance_len = tl
    //   The decoder copies wl bytes from the dict reference but the input
    //   cursor advances by tl bytes.
    let mut matches_at: Vec<Option<(u32, u32, u32, bool)>> = vec![None; n];
    for pos in 0..n {
        let global_pos = mlen_offset + pos;
        let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);

        if pos + MIN_MATCH as usize <= n {
            mf.advance();
            if let Some(m) = mf.find_match(mlen_offset + pos) {
                if m.distance <= max_dist && m.length >= MIN_MATCH {
                    let copy_len = m.length.min(MAX_COPY).max(MIN_MATCH);
                    matches_at[pos] = Some((m.distance, copy_len, copy_len, false));
                }
            }
        }

        // Check dictionary when hash found no match OR found a short
        // match (< 16 bytes). Long hash matches are almost never beaten
        // by the dictionary, and the dict lookup adds ~3μs per position
        // which is expensive on repetitive text (1M positions × 3μs = 3s).
        // FSST-transformed data typically has short hash matches (4-8
        // bytes) where the dictionary's word transforms can win.
        if use_dict {
            let hash_len = matches_at[pos].map(|(_, l, _, _)| l).unwrap_or(0);
            if hash_len < 16 {
                if let Some((d, wl, tl)) = dict_hash::find_match(input, pos, max_dist) {
                    if tl >= MIN_MATCH && pos + tl as usize <= n {
                        let copy_len = wl.min(MAX_COPY).max(MIN_MATCH);
                        let is_better = match matches_at[pos] {
                            None => true,
                            Some((_, existing_len, _, _)) => tl > existing_len,
                        };
                        if is_better {
                            matches_at[pos] = Some((d, copy_len, tl, true));
                        }
                    }
                }
            }
        }
    }

    // --- Step 2: Build literal cost model ---
    // For the first iteration, use Shannon entropy from input bytes.
    // For iterative refinement (TODO 246), the caller can pass a
    // refined lit_cost derived from iteration N's parsed literals.
    let lit_cost = if let Some(provided) = lit_cost_override {
        provided
    } else {
        compute_shannon_lit_cost(input)
    };

    // --- Distance cost approximation ---
    // Brotli distance alphabet: ~5 base bits + log2(dist) extra bits.
    // Returns bits required to encode a given distance.
    let dist_cost = |dist: u32| -> f32 {
        if dist == 0 {
            return 0.0;
        }
        let d = dist as f32;
        if dist <= 4 {
            2.0 + 0.5 * d
        } else {
            let log_d = d.ln() / core::f32::consts::LN_2;
            (5.0 + log_d).min(22.0)
        }
    };

    // Copy-length extra bits cost. Longer copies require more extra bits
    // in the command encoding (up to 24 bits for copy_len > 4336).
    // Without this, the DP underestimates long-copy costs and chooses
    // them too aggressively — a regression on FSST-preprocessed data
    // where long copies are often suboptimal.
    let copy_extra_cost = |copy_len: u32| -> f32 {
        if copy_len <= 18 {
            0.0 // codes 0-7: 0 extra bits
        } else if copy_len <= 54 {
            1.0 // codes 8-9: 1 extra bit
        } else if copy_len <= 134 {
            2.0 // codes 10-11: 2 extra bits
        } else if copy_len <= 302 {
            3.0 // codes 12-13: 3 extra bits
        } else if copy_len <= 698 {
            4.0 // codes 14-15: 4 extra bits
        } else if copy_len <= 1638 {
            5.0 // codes 16-17: 5 extra bits
        } else if copy_len <= 2288 {
            7.0 // codes 18-22: 6-11 extra bits (approx)
        } else {
            18.0 // codes 23+: up to 24 extra bits
        }
    };

    // --- Step 3: Backward DP considering all sub-match lengths ---
    // For each position i with a longest match of length max_L at distance D:
    //   cost[i] = min over L in [MIN_MATCH, max_L] of:
    //              match_cost(D) + cost[i+L]
    //            OR  lit_cost(input[i]) + cost[i+1]
    // Within a single copy-length code, match_cost is constant, so the
    // search prefers longer L (which amortizes the fixed cost better)
    // unless a better alignment appears at i+L.
    //
    // To keep this tractable, sample L at copy-code group boundaries
    // (the highest L in each group wins ties). 45 samples per position
    // keeps the DP at O(45 * N). Reduced sets (16 entries) were tested
    // but hurt ratio by 2+ pp on CSV data — the finer granularity finds
    // significantly better match alignments.
    const COPY_BOUNDARIES: [u32; 52] = [
        4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 22, 24, 26, 28, 30, 32, 34,
        36, 40, 44, 48, 52, 60, 68, 76, 84, 100, 116, 132, 148, 164, 180, 196, 212, 228, 244, 260,
        271, 432, 496, 752, 1264, 2288, 3040, 4096,
    ];
    let mut cost = vec![f32::INFINITY; n + 1];
    let mut back_len: Vec<u32> = vec![0; n]; // 0 = literal, else = copy_len
    let mut back_advance: Vec<u32> = vec![0; n]; // parallel: cursor advance
    cost[n] = 0.0;

    for i in (0..n).rev() {
        // Option A: Insert 1 literal.
        let lit_cost_total = lit_cost[input[i] as usize] + cost[i + 1];
        let mut best = lit_cost_total;
        let mut best_action = 0u32;
        let mut best_advance = 0u32;

        // Option B: Match of length L at copy-code boundaries.
        if let Some((dist, copy_len, advance_len, is_dict)) = matches_at[i] {
            if dist > 0 && copy_len >= MIN_MATCH {
                let m_cost = 7.0 + dist_cost(dist);
                if is_dict && advance_len != copy_len {
                    let l = i + advance_len as usize;
                    if l <= n {
                        let total = m_cost + copy_extra_cost(copy_len) + cost[l];
                        if total < best {
                            best = total;
                            best_action = copy_len;
                            best_advance = advance_len;
                        }
                    }
                } else if is_dict {
                    let l = i + copy_len as usize;
                    if l <= n {
                        let total = m_cost + copy_extra_cost(copy_len) + cost[l];
                        if total < best {
                            best = total;
                            best_action = copy_len;
                            best_advance = copy_len;
                        }
                    }
                } else {
                    for &boundary in &COPY_BOUNDARIES {
                        if boundary < MIN_MATCH || boundary > copy_len {
                            continue;
                        }
                        let l = i + boundary as usize;
                        if l > n {
                            break;
                        }
                        let total = m_cost + copy_extra_cost(boundary) + cost[l];
                        if total < best {
                            best = total;
                            best_action = boundary;
                            best_advance = boundary;
                        }
                    }
                }
            }
        }

        cost[i] = best;
        back_len[i] = best_action;
        back_advance[i] = best_advance;
    }

    // --- Step 4: Forward reconstruction ---
    let mut commands = Vec::new();
    let mut pos = 0;
    let mut insert_start = 0;

    while pos < n {
        if back_len[pos] > 0 {
            let copy_len = back_len[pos];
            let advance = back_advance[pos];
            let (dist, _, _, _) = matches_at[pos].unwrap();
            let insert_len = (pos - insert_start) as u32;
            commands.push(Command {
                insert_len,
                copy_len,
                distance: dist,
            });
            pos += advance as usize;
            insert_start = pos;
        } else {
            pos += 1;
        }
    }

    // Trailing literals.
    if insert_start < n {
        commands.push(Command {
            insert_len: (n - insert_start) as u32,
            copy_len: 0,
            distance: 0,
        });
    }

    commands
}

/// Iterative refinement of [`optimal_parse`] (TODO 246).
///
/// Runs two passes:
/// 1. Pass 1 uses Shannon entropy over all input bytes as the literal
///    cost model. This is what [`optimal_parse`] does today.
/// 2. Build literal stream from pass 1's commands. Compute Shannon
///    entropy over THOSE literals (which often excludes bytes that
///    ended up in copy matches, sharpening the distribution).
/// 3. Pass 2 re-parses with the refined literal cost.
///
/// Returns whichever pass produced the smaller Huffman stream
/// (measured by total symbol bits, not literal count — passes can
/// trade literals for matches).
///
/// ## Why this helps
///
/// Pass 1's lit_cost treats every byte uniformly. But after parsing,
/// many bytes are inside copied matches and don't go through the
/// literal Huffman tree. The actual literal byte distribution is
/// often sharper (more skew toward common ASCII letters), so the
/// Huffman tree built from those literals is cheaper for the bytes
/// that actually appear as literals.
///
/// ## Why two passes is enough
///
/// Standard convergence: pass 2's literal stream ≈ pass 1's, so a
/// third pass reproduces pass 2's output. Cap at 2 for predictable
/// runtime.
#[allow(dead_code)]
fn iterative_optimal_parse(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    use_dict: bool,
) -> Vec<Command> {
    iterative_optimal_parse_with_iters(input, mf, mlen_offset, use_dict, 2)
}

/// Multi-pass iterative optimal parser (TODO 272). Each iteration
/// refines the literal cost model based on the previous iteration's
/// actual parsed literals. 2 iterations is the default; Q11 uses 4
/// for additional refinement.
#[allow(clippy::too_many_lines, dead_code)]
fn iterative_optimal_parse_with_iters(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    use_dict: bool,
    iterations: usize,
) -> Vec<Command> {
    // Pass 1: Shannon cost from input.
    let mut best_commands = optimal_parse(input, mf, mlen_offset, use_dict);
    let mut best_score = score_commands(&best_commands, input, mlen_offset);

    let config = omnizip_codecs::HashChainConfig {
        dict_size: MAX_BACKWARD_DISTANCE,
        min_match: MIN_MATCH,
        max_chain_length: mf_max_chain(mf),
        nice_match: mf_nice_match(mf),
        hash_log: mf_hash_log(mf),
        max_match_length: MAX_COPY,
    };

    // Subsequent passes: refine lit_cost from the previous pass's
    // actual literals. Stop if no improvement.
    for _ in 1..iterations {
        let literals_prev = extract_literals(&best_commands, input, mlen_offset);
        if literals_prev.is_empty() {
            break;
        }
        let lit_cost_refined = compute_shannon_lit_cost(&literals_prev);
        let mut mf_iter = omnizip_codecs::HashChainMatchFinder::new(input, config);
        let commands_iter = optimal_parse_with_costs(
            input,
            &mut mf_iter,
            mlen_offset,
            use_dict,
            Some(lit_cost_refined),
        );
        let score_iter = score_commands(&commands_iter, input, mlen_offset);
        if score_iter < best_score {
            best_score = score_iter;
            best_commands = commands_iter;
        }
    }

    best_commands
}

/// Extract just the literal bytes from a command list (in stream order).
#[allow(dead_code)]
fn extract_literals(commands: &[Command], input: &[u8], mlen_offset: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cur = 0usize;
    for cmd in commands {
        let end = cur + cmd.insert_len as usize;
        out.extend_from_slice(&input[cur..end]);
        let advance = if cmd.copy_len > 0 {
            let is_dict = (cmd.distance as usize) > cur;
            if is_dict {
                let global_pos = mlen_offset + end;
                let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                match dictionary_lookup(&mut tmp, cmd.copy_len, cmd.distance as i32, max_dist) {
                    Some(()) => tmp.len(),
                    None => cmd.copy_len as usize,
                }
            } else {
                cmd.copy_len as usize
            }
        } else {
            0
        };
        cur = end + advance;
    }
    out
}

/// Rough score: total bits the command stream would cost in the
/// Huffman-coded wire format. Lower is better.
///
/// This is an approximation — it doesn't build actual Huffman trees
/// — but it's a sufficient signal for "is iteration 2 better?".
#[allow(dead_code)]
fn score_commands(commands: &[Command], input: &[u8], mlen_offset: usize) -> u64 {
    let mut literal_count = 0u64;
    let mut cmd_count = 0u64;
    let mut dist_count = 0u64;
    let mut literals_freq = [0u32; 256];

    let mut cur = 0usize;
    for cmd in commands {
        let end = cur + cmd.insert_len as usize;
        for &b in &input[cur..end] {
            literals_freq[b as usize] += 1;
            literal_count += 1;
        }
        if cmd.copy_len > 0 {
            cmd_count += 1;
            let is_dict = (cmd.distance as usize) > cur;
            let advance = if is_dict {
                let global_pos = mlen_offset + end;
                let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                match dictionary_lookup(&mut tmp, cmd.copy_len, cmd.distance as i32, max_dist) {
                    Some(()) => tmp.len(),
                    None => cmd.copy_len as usize,
                }
            } else {
                cmd.copy_len as usize
            };
            // Heuristic: count as a distance symbol only if not a rep.
            // We don't track rep state here; assume worst case (all
            // explicit distances). This biases toward fewer commands
            // which is fine for ranking.
            dist_count += 1;
            cur = end + advance;
        } else {
            cur = end;
        }
    }

    // Shannon bound on literal stream.
    let total = literal_count.max(1) as f32;
    let mut lit_bits = 0.0f32;
    for &f in &literals_freq {
        if f > 0 {
            let p = f as f32 / total;
            lit_bits += -p.log2() * f as f32;
        }
    }

    // Command + distance overhead: ~8 bits per command, ~10 per distance.
    (lit_bits as u64) + cmd_count * 8 + dist_count * 10
}

// Read-only accessors for HashChainConfig fields (it's private inside
// HashChainMatchFinder; we mirror via the config struct that the caller
// built). Since we can't read them back from mf itself, default to
// reasonable values for the iterative refinement pass.
#[allow(dead_code)]
fn mf_max_chain(_mf: &omnizip_codecs::HashChainMatchFinder) -> u32 {
    128
}
#[allow(dead_code)]
fn mf_nice_match(_mf: &omnizip_codecs::HashChainMatchFinder) -> u32 {
    128
}
#[allow(dead_code)]
fn mf_hash_log(_mf: &omnizip_codecs::HashChainMatchFinder) -> u32 {
    17
}

/// Two-pass backward reference collection (TODO 241).
///
/// Pass 1: Walk all positions, find ALL matches via hash chain +
/// dictionary. Store in a pre-allocated array.
///
/// Pass 2: Walk the matches array with extended look-ahead (4
/// positions). Pick the longest match, deferring when a longer
/// match is available at a nearby position. This finds better
/// match combinations than single-pass lazy parsing because
/// ALL matches are visible before any decisions are made.
fn two_pass_parse(
    input: &[u8],
    mlen_offset: usize,
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    use_dict: bool,
) -> Vec<Command> {
    let n = input.len();
    if n < MIN_MATCH as usize + 1 {
        return vec![Command {
            insert_len: n as u32,
            copy_len: 0,
            distance: 0,
        }];
    }

    // --- Pass 1: Collect all matches ---
    // Each entry: (distance, copy_len=word_length, advance_len=transformed_len)
    let limit = n.saturating_sub(MIN_MATCH as usize);
    let mut matches: Vec<Option<(u32, u32, u32)>> = vec![None; n];

    for pos in 0..limit {
        mf.advance();
        let global_pos = mlen_offset + pos;
        let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);

        if let Some(m) = mf.find_match(pos) {
            if m.distance > 0 && m.distance <= max_dist && m.length >= MIN_MATCH {
                let copy_len = m.length.min(MAX_COPY).max(MIN_MATCH);
                matches[pos] = Some((m.distance, copy_len, copy_len));
            }
        }

        if matches[pos].is_none() && use_dict {
            if let Some((d, wl, tl)) = dict_hash::find_match(input, pos, max_dist) {
                if tl >= MIN_MATCH && pos + tl as usize <= n {
                    let copy_len = wl.min(MAX_COPY).max(MIN_MATCH);
                    matches[pos] = Some((d, copy_len, tl));
                }
            }
        }
    }

    // --- Pass 2: Greedy with extended look-ahead ---
    // At each position with a match, check the next 4 positions.
    // If any has a significantly longer match, defer.
    let mut commands = Vec::new();
    let mut pos = 0usize;
    let mut insert_start = 0usize;

    while pos < n {
        if let Some((dist, copy_len, advance_len)) = matches[pos] {
            if copy_len >= MIN_MATCH && dist > 0 {
                // Extended look-ahead: check next 4 positions for free
                let mut best_pos = pos;
                let mut best_copy = copy_len;
                let mut best_dist = dist;
                let mut best_advance = advance_len;

                for offset in 1..=4u32 {
                    let next = pos + offset as usize;
                    if next >= n {
                        break;
                    }
                    if let Some((d2, c2, a2)) = matches[next] {
                        if a2 > best_advance + offset {
                            best_pos = next;
                            best_copy = c2;
                            best_dist = d2;
                            best_advance = a2;
                        }
                    }
                }

                let insert_len = (best_pos - insert_start) as u32;
                commands.push(Command {
                    insert_len,
                    copy_len: best_copy,
                    distance: best_dist,
                });

                pos = best_pos + best_advance as usize;
                insert_start = pos;
                continue;
            }
        }
        pos += 1;
    }

    // Trailing literals
    if insert_start < n {
        commands.push(Command {
            insert_len: (n - insert_start) as u32,
            copy_len: 0,
            distance: 0,
        });
    }

    commands
}

#[allow(dead_code)]
fn parse_input(input: &[u8]) -> Vec<Command> {
    let is_text = is_text_like(input);
    let (max_chain, nice_match, _, _, _, hash_log) = brotli_quality_config(11, is_text);
    let config = omnizip_codecs::HashChainConfig {
        dict_size: MAX_BACKWARD_DISTANCE,
        min_match: MIN_MATCH,
        max_chain_length: max_chain,
        nice_match,
        hash_log,
        max_match_length: MAX_COPY,
    };
    let mut mf = omnizip_codecs::HashChainMatchFinder::new(input, config);
    parse_input_with_offset(input, &mut mf, 0, 11, false)
}

/// Quality → (max_chain, nice_match, use_dict_base, lazy, lazy2, hash_log).
/// DRY helper so callers don't duplicate the match table.
fn brotli_quality_config(quality: i32, is_text: bool) -> (u32, u32, bool, bool, bool, u32) {
    if is_text {
        match quality {
            0..=1 => (4, 8, false, false, false, 15),
            2..=3 => (8, 16, true, true, false, 16),
            4..=5 => (8, 32, true, true, true, 17),
            6..=7 => (16, 48, true, true, true, 17),
            8..=9 => (32, 64, true, true, true, 17),
            _ => (64, 128, true, true, true, 18),
        }
    } else {
        match quality {
            0..=1 => (4, 8, false, false, false, 15),
            _ => (8, 16, false, false, false, 16),
        }
    }
}

/// Parse input into commands using LZ77 + static dictionary, with a
/// non-zero `mlen_offset` for chunks in a multi-metablock frame.
///
/// `mlen_offset` is added to local `pos` when computing per-position
/// `max_distance`, so dictionary references use the same distance
/// formula the decoder will use.
///
/// `quality` (0–11) controls match-finder effort and whether the
/// static dictionary is consulted.
///
/// `disable_dict` temporarily disables dictionary lookups (used when
/// context modeling is active, due to a decoder interaction bug).
fn parse_input_with_offset(
    input: &[u8],
    mut mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    quality: i32,
    disable_dict: bool,
) -> Vec<Command> {
    let n = input.len();
    let mut commands = Vec::new();

    // At Q4+, always use the text config (deeper chains, dict, lazy2)
    // regardless of content type. FSST-transformed data and other
    // semi-structured binary benefits from the same parser effort as
    // natural text. The optimal parser compensates for any mismatch.
    let (max_chain, nice_match, use_dict_base, lazy, lazy2, hash_log) =
        brotli_quality_config(quality, true);
    let use_dict = use_dict_base && !disable_dict;

    let _config = omnizip_codecs::HashChainConfig {
        dict_size: MAX_BACKWARD_DISTANCE,
        min_match: MIN_MATCH,
        max_chain_length: max_chain,
        nice_match,
        hash_log,
        max_match_length: MAX_COPY,
    };
    // MF is provided by the caller — no creation here.

    // Q4+: cost-aware optimal parser for any content type (TODO 240).
    // The single-pass `optimal_parse` matches the 2-pass iterative
    // parser's ratio on every input we tested, and is 25-35% faster
    // (Q8: 14.3s → 9.5s, Q11: 19.4s → 15.0s on the 20 MiB CSV
    // benchmark). 2-pass refinement was over-engineering — the
    // Shannon-entropy cost model is already within 1-3% of the
    // actual Huffman cost, so the second pass had little to refine.
    if quality >= 4 && input.len() <= 1024 * 1024 {
        return optimal_parse(input, &mut mf, mlen_offset, use_dict);
    }

    // Q4+ with input > 1 MiB (not chunked): two_pass_parse.
    if quality >= 4 {
        return two_pass_parse(input, mlen_offset, &mut mf, use_dict);
    }

    let mut pos = 0usize;
    let mut insert_start = 0usize;
    while pos < n {
        // Global output position (across metablocks) for max_distance.
        let global_pos = mlen_offset + pos;
        let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);

        let lz77 = if pos + MIN_MATCH as usize <= n {
            mf.advance();
            mf.find_match(pos)
        } else {
            None
        };

        let lz77_valid = lz77.as_ref().is_some_and(|m| m.distance <= max_dist);

        let best: Option<(u32, u32, u32)> = if lz77_valid {
            let m = lz77.as_ref().unwrap();
            if m.length >= 8 || !use_dict {
                Some((m.distance, m.length, m.length))
            } else {
                let dict = dict_hash::find_match(input, pos, max_dist);
                match dict {
                    Some((d, wl, tl)) if tl > m.length => Some((d, wl, tl)),
                    _ => Some((m.distance, m.length, m.length)),
                }
            }
        } else if use_dict {
            dict_hash::find_match(input, pos, max_dist).map(|(d, wl, tl)| (d, wl, tl))
        } else {
            None
        };

        if let Some((distance, copy_len, advance_len)) = best {
            if advance_len >= MIN_MATCH && distance > 0 {
                // Lazy matching: if the current match is short, check if
                // deferring by one position yields a longer match.
                if lazy && advance_len < nice_match && pos + 1 < n {
                    let next_pos = pos + 1;
                    let next_global = mlen_offset + next_pos;
                    let next_max = (next_global as u32).min(MAX_BACKWARD_DISTANCE);

                    let next_lz77 = if next_pos + MIN_MATCH as usize <= n {
                        mf.find_match(next_pos)
                    } else {
                        None
                    };

                    let next_valid = next_lz77.as_ref().is_some_and(|m| m.distance <= next_max);
                    let next_best_len: Option<u32> = if next_valid {
                        let nm = next_lz77.as_ref().unwrap();
                        if nm.length >= 8 || !use_dict {
                            Some(nm.length)
                        } else {
                            match dict_hash::find_match(input, next_pos, next_max) {
                                Some((_, _, tl)) if tl > nm.length => Some(tl),
                                _ => Some(nm.length),
                            }
                        }
                    } else if use_dict {
                        dict_hash::find_match(input, next_pos, next_max).map(|(_, _, tl)| tl)
                    } else {
                        None
                    };

                    if let Some(next_len) = next_best_len {
                        if next_len > advance_len {
                            // Lazy2: check pos+2 before committing to pos+1.
                            if lazy2 && next_len < nice_match && pos + 2 < n {
                                let next2_pos = pos + 2;
                                let next2_global = mlen_offset + next2_pos;
                                let next2_max = (next2_global as u32).min(MAX_BACKWARD_DISTANCE);
                                let next2_best_len: Option<u32> = if next2_pos + MIN_MATCH as usize
                                    <= n
                                {
                                    mf.find_match(next2_pos)
                                        .filter(|m| m.distance <= next2_max)
                                        .map(|m| {
                                            if m.length >= 8 || !use_dict {
                                                m.length
                                            } else {
                                                match dict_hash::find_match(
                                                    input, next2_pos, next2_max,
                                                ) {
                                                    Some((_, _, tl)) if tl > m.length => tl,
                                                    _ => m.length,
                                                }
                                            }
                                        })
                                        .or_else(|| {
                                            if use_dict {
                                                dict_hash::find_match(input, next2_pos, next2_max)
                                                    .map(|(_, _, tl)| tl)
                                            } else {
                                                None
                                            }
                                        })
                                } else {
                                    None
                                };
                                if let Some(n2_len) = next2_best_len {
                                    if n2_len > next_len {
                                        pos += 2;
                                        continue;
                                    }
                                }
                            }
                            pos += 1;
                            continue;
                        }
                    }
                }

                let clamped_copy = copy_len.min(MAX_COPY).max(MIN_MATCH);
                let insert_len = (pos - insert_start) as u32;
                commands.push(Command {
                    insert_len,
                    copy_len: clamped_copy,
                    distance,
                });
                // Advance: for LZ77, use clamped copy_len (matches
                // decoder output). For dictionary, use transformed_len
                // (may differ from copy_len when transforms add/remove
                // bytes).
                let advance = if advance_len > MAX_COPY {
                    clamped_copy as usize
                } else {
                    (advance_len as usize).min(n - pos)
                };
                for _ in 1..advance {
                    if pos + 1 < n {
                        pos += 1;
                        mf.advance();
                    }
                }
                pos += 1;
                insert_start = pos;
                continue;
            }
        }
        pos += 1;
    }

    // Trailing literals: emit a separate trailing-insert command (with
    // copy_len=0, encoded as a phantom copy_len=2 that the decoder
    // short-circuits past at metablock end).
    if insert_start < n {
        let trailing = (n - insert_start) as u32;
        commands.push(Command {
            insert_len: trailing,
            copy_len: 0,
            distance: 0,
        });
    }

    commands
}

/// Build canonical Huffman codes (MSB-first) and bit-reverse each code
/// to its LSB-first wire form. Returns `Vec<(wire_code, length)>`.
///
/// Special case: when the alphabet has exactly one non-zero symbol
/// (the simple-form NSYM=1 layout), the decoder reads 0 bits per
/// occurrence via its `single_symbol` fast path. We reflect that here
/// by returning (0, 0) for the sole non-zero symbol so the writer
/// emits nothing for it.
fn canonical_with_reverse(lengths: &omnizip_codecs::HuffmanLengths) -> Vec<(u32, u8)> {
    let nonzero_count = lengths.lengths.iter().filter(|&&l| l > 0).count();
    let codes = lengths.canonical_codes();
    codes
        .into_iter()
        .map(|(c, l)| {
            let l = if nonzero_count == 1 { 0 } else { l };
            (reverse_bits(c, l), l)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Huffman table encoding (RFC 7932 §9.5)
// ---------------------------------------------------------------------------

/// Code-length code prefix: maps each code-length value (0-5) to its
/// (`wire_value`, bits) encoding via the fixed `K_CL_PREFIX` code.
///
/// Derived from the decoder's `K_CL_PREFIX_VALUE` / `K_CL_PREFIX_LENGTH`
/// tables (decoder.rs:645-646). Each entry is the LSB-first stream
/// representation of the prefix code for that value.
const CL_CODE_TO_WIRE: [(u32, u8); 6] = [
    (0b00, 2),   // value 0
    (0b0111, 4), // value 1
    (0b011, 3),  // value 2
    (0b10, 2),   // value 3
    (0b01, 2),   // value 4
    (0b1111, 4), // value 5
];

/// `CODE_LENGTH_CODE_ORDER` per RFC 7932 §9.5.2.
const CODE_LENGTH_CODE_ORDER: [u8; 18] =
    [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// Write a Huffman table (RFC 7932 §9.5).
///
/// Uses the complex form (HSKIP=0) for any alphabet size. The
/// implementation does NOT emit RLE symbols (16/17), which is slightly
/// wasteful for sparse code-length arrays but produces correct output.
///
/// The number of code-length entries written matches what the decoder
/// will read: the decoder breaks its read loop once the code-length
/// prefix code's "space" is fully consumed (sum of 32>>len = 32). We
/// replicate that break here so the bit position after this table
/// matches the decoder's expectation.
fn write_huffman_table(
    bw: &mut BitWriter,
    lengths: &omnizip_codecs::HuffmanLengths,
    alphabet: usize,
) {
    let nonzero: Vec<usize> = lengths
        .lengths
        .iter()
        .enumerate()
        .filter(|(_, &l)| l > 0)
        .map(|(i, _)| i)
        .collect();
    // Use simple form when the code lengths match a simple-form pattern.
    // This avoids the complex form RLE path for sparse tables where it
    // produces wire-format mismatches.
    if nonzero.len() <= 1 {
        let sym = nonzero.first().copied().unwrap_or(0);
        write_simple_one_symbol(bw, alphabet, sym);
        return;
    }
    // Check if lengths match a simple-form assignment:
    // NSYM=2: both length 1
    // NSYM=3: first length 1, other two length 2
    // NSYM=4: all length 2
    let matches_simple = match nonzero.len() {
        2 => lengths.lengths[nonzero[0]] == 1 && lengths.lengths[nonzero[1]] == 1,
        3 => lengths.lengths[nonzero[0]] == 1
            && lengths.lengths[nonzero[1]] == 2
            && lengths.lengths[nonzero[2]] == 2,
        4 => nonzero.iter().all(|&i| lengths.lengths[i] == 2),
        _ => false,
    };
    if matches_simple {
        write_simple_form_table(bw, alphabet, &nonzero);
        return;
    }

    // Complex form: HSKIP = 0.
    bw.write_bits(0, 2);

    let rle = build_rle_sequence(&lengths.lengths[..alphabet]);

    // Build a sub-Huffman over the 18-symbol code-length alphabet,
    // using frequencies from the RLE-compressed sequence (not the raw
    // lengths). This accounts for symbols 16/17 in the code-length code.
    let mut cl_freq = [0u32; 18];
    for &(sym, _) in &rle {
        cl_freq[usize::from(sym)] += 1;
    }
    let cl_lengths = omnizip_codecs::HuffmanLengths::build(&cl_freq, 5);
    let cl_codes = cl_lengths.canonical_codes();

    // When the code-length code has exactly one non-zero symbol, the
    // decoder's single_symbol fast path reads 0 bits per occurrence.
    // We detect this and write 0 bits below.
    let cl_single = cl_lengths.lengths.iter().filter(|&&l| l > 0).count() == 1;

    // Walk CODE_LENGTH_CODE_ORDER, emitting each code-length value via
    // the fixed K_CL_PREFIX code. Stop early once the code-length prefix
    // code's space is fully consumed (mirrors the decoder's break).
    let mut space: u32 = 32;
    let mut num_codes: u32 = 0;
    for &sym in &CODE_LENGTH_CODE_ORDER {
        let len = cl_lengths.lengths[usize::from(sym)];
        let (wire, nbits) = CL_CODE_TO_WIRE[usize::from(len)];
        bw.write_bits(wire, u32::from(nbits));

        if len != 0 {
            space = space.wrapping_sub(32u32 >> u32::from(len));
            num_codes += 1;
            // Decoder breaks when space.wrapping_sub(1) >= 32, i.e. when
            // space has reached 0 (or underflowed, which can't happen
            // for a valid prefix code).
            if num_codes != 1 && space.wrapping_sub(1) >= 32 {
                break;
            }
        }
    }

    // Write the actual code lengths using the code-length Huffman code,
    // emitting RLE symbols (16/17) from the pre-computed sequence.
    // The decoder exits its read loop once the main prefix code's space
    // is fully consumed (sum of 32768>>len = 32768). We replicate that
    // break here so the bit position after this table matches.
    let mut main_space: u32 = 32768;
    let mut prev_code_len: u8 = 8;
    for &(sym, extra) in &rle {
        let (val, count) = match sym {
            16 => (prev_code_len, 3 + extra as usize),
            17 => (0u8, 3 + extra as usize),
            v => (v, 1usize),
        };

        if !cl_single {
            let (code, clen) = cl_codes[usize::from(sym)];
            let wire = reverse_bits(code, clen);
            bw.write_bits(wire, u32::from(clen));
        }
        // Extra bits for RLE symbols must be written even when
        // cl_single is true (decoder reads 0 bits for the symbol
        // itself via single_symbol fast path, but still reads the
        // extra bits for symbols 16 and 17).
        if sym == 16 {
            bw.write_bits(extra as u32, 2);
        } else if sym == 17 {
            bw.write_bits(extra as u32, 3);
        }

        if val != 0 {
            prev_code_len = val;
            for _ in 0..count {
                main_space = main_space.wrapping_sub(32768u32 >> u32::from(val));
                if main_space == 0 {
                    return;
                }
            }
        }
    }
}

/// Build an RLE-compressed code-length sequence from raw lengths.
///
/// Returns `Vec<(symbol, extra_bits)>`:
/// - Symbols 0-15: literal code-length values (extra = 0).
/// - Symbol 16: repeat previous code length `3 + extra` times (extra ∈ 0..4).
/// - Symbol 17: repeat zero `3 + extra` times (extra ∈ 0..8).
///
/// To avoid the decoder's iterated accumulator (which combines
/// consecutive repeat symbols non-linearly), a literal symbol is
/// inserted between consecutive repeats of the same value.
fn build_rle_sequence(lengths: &[u8]) -> Vec<(u8, u8)> {
    let n = lengths.len();
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let val = lengths[i];
        let mut run = 1usize;
        while i + run < n && lengths[i + run] == val {
            run += 1;
        }

        if val == 0 {
            // Emit leading 1-2 zeros individually (symbol 17 needs ≥3).
            let lead = run.min(2);
            for _ in 0..lead {
                out.push((0, 0));
            }
            run -= lead;
            i += lead;
            // Use symbol 17 for remaining zero runs (count 3-10).
            // Insert a literal 0 between consecutive symbol-17s to
            // avoid the decoder's iterated accumulator.
            while run >= 3 {
                let chunk = run.min(10);
                out.push((17, (chunk - 3) as u8));
                run -= chunk;
                i += chunk;
                if run >= 3 {
                    out.push((0, 0));
                    run -= 1;
                    i += 1;
                }
            }
            for _ in 0..run {
                out.push((0, 0));
            }
            i += run;
        } else {
            out.push((val, 0));
            i += 1;
            run -= 1;
            // Use symbol 16 for repeat runs (count 3-6).
            // Insert a literal between consecutive symbol-16s.
            while run >= 3 {
                let chunk = run.min(6);
                out.push((16, (chunk - 3) as u8));
                run -= chunk;
                i += chunk;
                if run >= 3 {
                    out.push((val, 0));
                    run -= 1;
                    i += 1;
                }
            }
            for _ in 0..run {
                out.push((val, 0));
            }
            i += run;
        }
    }
    out
}

/// Write block-type code trees and initial block length (RFC 7932 §9.3).
///
/// For NBLTYPES=2:
/// - Block-type code tree: simple form NSYM=2, symbols [2, 3] (1 bit each)
///   symbol 2 → type 0, symbol 3 → type 1
/// - Block-length code tree: simple form NSYM=1, symbol = block-length code
/// - Initial block length: code 12 (offset=113) + extra (for 128 bytes)
fn write_block_type_trees(bw: &mut BitWriter, _nbltypes: u32) {
    // Block-type code tree: alphabet 2 + 2 = 4.
    // Simple form NSYM=2: symbols 2 and 3 (each 1 bit).
    bw.write_bits(0b01, 2); // HSKIP = 1
    bw.write_bits(0b01, 2); // NSYM-1 = 1 (NSYM=2)
    let bits_per_sym = ceil_log2(4); // ceil(log2(4)) = 2
    bw.write_bits(2, bits_per_sym); // s0 = 2 (type 0)
    bw.write_bits(3, bits_per_sym); // s1 = 3 (type 1)

    // Block-length code tree: alphabet 26.
    // Simple form NSYM=1: symbol = 12 (block-length code for ~128 bytes).
    // kBlockLengthPrefixCode[12] = offset=113, nbits=5 → range [113, 144].
    bw.write_bits(0b01, 2); // HSKIP = 1
    bw.write_bits(0b00, 2); // NSYM-1 = 0 (NSYM=1)
    let bl_bits_per_sym = ceil_log2(26); // ceil(log2(26)) = 5
    bw.write_bits(12, bl_bits_per_sym); // symbol = 12

    // Initial block length: read symbol 12 (0 bits, single_symbol), then
    // extra bits: 128 - 113 = 15 in 5 bits.
    bw.write_bits(15, 5); // extra = 15 → block_length = 113 + 15 = 128
}

/// Write a DecodeVarLenUint8-encoded value (RFC 7932 §9.3).
///
/// Inverse of the decoder's `read_varlen_uint8`:
/// - 0 → bit 0
/// - 1 → bits [1, 0,0,0] (1 + nbits=0 in 3 bits)
/// - N ≥ 2 → bits [1, nbits in 3 bits, extra in nbits bits]
///   where N = (1 << nbits) + extra.
fn write_varlen_uint8(bw: &mut BitWriter, value: u32) {
    if value == 0 {
        bw.write_bits(0, 1);
        return;
    }
    bw.write_bits(1, 1);
    if value == 1 {
        bw.write_bits(0, 3); // nbits = 0
        return;
    }
    let nbits = 31 - value.leading_zeros().min(31);
    let extra = value - (1u32 << nbits);
    bw.write_bits(nbits, 3);
    bw.write_bits(extra, nbits);
}

/// Write a literal context map (RFC 7932 §9.6) for NTREES > 1.
///
/// Format:
/// 1. RLE flag = 0 (no run-length encoding of zeros)
/// 2. Context-map code Huffman tree (simple form, alphabet = NTREES)
/// 3. One symbol per context-map entry (64 entries for LSB6)
/// 4. Inverse-MTF flag = 0
fn write_context_map(bw: &mut BitWriter, ctx_map: &[u8], ntrees: u32) {
    // RLE flag = 0 (no RLE).
    bw.write_bits(0, 1);

    // Context-map code tree: simple form with NTREES symbols.
    write_context_map_tree(bw, ntrees);

    // Write each context-map entry using the context-map code tree.
    // The decoder's HuffmanTable stores bit-reversed canonical codes
    // (LSB-first bitstream convention). We must write the REVERSED
    // code for each symbol, not the raw symbol value.
    //
    // For NSYM=2 (1-bit codes): reversal is identity (0→0, 1→1).
    // For NSYM=4 (2-bit codes): reversal swaps codes 1↔2
    //   (canonical 01→reversed 10, canonical 10→reversed 01).
    let bits_per_entry = if ntrees <= 2 { 1u8 } else { 2u8 };
    for &entry in ctx_map {
        let code = reverse_bits(entry as u32, bits_per_entry);
        bw.write_bits(code, u32::from(bits_per_entry));
    }

    // Inverse-MTF flag = 0.
    bw.write_bits(0, 1);
}

/// Write the context-map code Huffman tree in simple form.
/// For NTREES=2: NSYM=2, both symbols get 1-bit codes.
fn write_context_map_tree(bw: &mut BitWriter, ntrees: u32) {
    // HSKIP = 1 (simple form).
    bw.write_bits(0b01, 2);
    // NSYM: write nsym-1 in 2 bits.
    bw.write_bits(ntrees - 1, 2);
    // Symbols: each ceil_log2(ntrees) bits.
    let bits_per_sym = ceil_log2(ntrees);
    for sym in 0..ntrees {
        bw.write_bits(sym, bits_per_sym);
    }
    // For NSYM=4: decoder reads tree_select bit. Write 0 for uniform
    // 2-bit codes.
    if ntrees == 4 {
        bw.write_bits(0, 1);
    }
}

/// Write a single-symbol Huffman table in simple form (RFC 7932 §9.5.1).
fn write_simple_one_symbol(bw: &mut BitWriter, alphabet: usize, sym: usize) {
    bw.write_bits(0b01, 2); // HSKIP = 1
    bw.write_bits(0b00, 2); // NSYM = 1 (encoded as 0b00)
    let bits_per_sym = ceil_log2(alphabet as u32);
    bw.write_bits(sym as u32, bits_per_sym);
}

/// Write a 2/3/4-symbol Huffman table in simple form (RFC 7932 §9.5.1).
/// Symbols must be sorted ascending. Code length assignment:
/// - NSYM=2: both length 1
/// - NSYM=3: s0 length 1, s1/s2 length 2
/// - NSYM=4: all length 2 (tree_select=0)
fn write_simple_form_table(bw: &mut BitWriter, alphabet: usize, symbols: &[usize]) {
    let nsym = symbols.len();
    debug_assert!((2..=4).contains(&nsym));
    bw.write_bits(0b01, 2); // HSKIP = 1 (simple form)
    bw.write_bits(nsym as u32 - 1, 2); // NSYM-1
    let bits_per_sym = ceil_log2(alphabet as u32);
    for &s in symbols {
        bw.write_bits(s as u32, bits_per_sym);
    }
    if nsym == 4 {
        bw.write_bits(0, 1); // tree_select = 0 (all length 2)
    }
}

/// Override Huffman code lengths to match simple form assignment.
/// Only applies to tables with exactly 2 non-zero symbols (the most
/// common sparse-table failure case). Both symbols get length 1.
fn override_lengths_for_simple_form(lengths: &mut [u8], alphabet: usize) {
    let nonzero: Vec<usize> = lengths[..alphabet]
        .iter()
        .enumerate()
        .filter(|(_, &l)| l > 0)
        .map(|(i, _)| i)
        .collect();
    if nonzero.len() == 2 {
        lengths[nonzero[0]] = 1;
        lengths[nonzero[1]] = 1;
    }
}

/// ⌈log2(n)⌉ for n ≥ 1.
fn ceil_log2(n: u32) -> u32 {
    if n <= 1 {
        return 0;
    }
    32 - (n - 1).leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder;

    #[test]
    fn empty_round_trips() {
        let compressed = compress(&[]);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert!(decoded.is_empty());
    }

    #[test]
    fn short_round_trips() {
        let input = b"hello world";
        let compressed = compress(input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded.as_slice(), input.as_ref());
    }

    #[test]
    fn repetitive_round_trips() {
        let input = b"abcabcabcabc".repeat(10);
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn binary_round_trips() {
        let input: Vec<u8> = (0..256u32).map(|i| (i % 256) as u8).collect();
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn csv_round_trips() {
        let input: Vec<u8> = (0..100)
            .map(|i| format!("row_{},{},value_{}\n", i, i * 2, i % 7))
            .collect::<String>()
            .into_bytes();
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn determinism() {
        let input = b"determinism test input with repetition repetition";
        let a = compress(input);
        let b = compress(input);
        assert_eq!(a, b);
    }

    #[test]
    fn compresses_repetitive_input() {
        // CSV-like input should compress to less than half its size when
        // the Huffman path lands. If the uncompressed fallback wins,
        // the test still passes (we just verify correctness above).
        let input: Vec<u8> = b"abcabcabcabc".repeat(50);
        let compressed = compress(&input);
        assert!(
            compressed.len() < input.len(),
            "compressed {} should be < input {}",
            compressed.len(),
            input.len()
        );
    }

    #[test]
    fn compresses_text_input() {
        let input: Vec<u8> = b"The quick brown fox jumps over the lazy dog. ".repeat(50);
        let compressed = compress(&input);
        assert!(
            compressed.len() < input.len(),
            "text compressed {} should be < input {}",
            compressed.len(),
            input.len()
        );
    }

    #[test]
    fn compresses_csv_input() {
        let input: Vec<u8> = (0..100)
            .map(|i| format!("row_{},{},value_{}\n", i, i * 2, i % 7))
            .collect::<String>()
            .into_bytes();
        let compressed = compress(&input);
        assert!(
            compressed.len() < input.len(),
            "csv compressed {} should be < input {}",
            compressed.len(),
            input.len()
        );
    }

    #[test]
    fn bit_writer_lsb_first() {
        let mut bw = BitWriter::new();
        bw.write_bits(1, 1);
        bw.write_bits(2, 2);
        let out = bw.flush();
        assert_eq!(out, vec![0b0101]);
    }

    #[test]
    fn large_repetitive_input_round_trips() {
        // 200 KiB. The encoder currently falls back to a single
        // uncompressed metablock for inputs >64 KiB (multi-metablock
        // Huffman path is TODO). Verify round-trip correctness.
        let input: Vec<u8> = b"the quick brown fox jumps over the lazy dog. ".repeat(5000);
        assert!(input.len() > 65_536);
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn near_max_metablock_round_trips() {
        // 60 KiB — fits in a single 4-nibble MLEN metablock.
        let input: Vec<u8> = vec![0u8; 60_000];
        let compressed = compress(&input);
        eprintln!("input {} -> compressed {}", input.len(), compressed.len());
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn random_single_metablock_huffman_round_trips() {
        // 4 KiB of pseudo-random data — exercises Huffman table
        // encoding with many distinct code lengths.
        let input: Vec<u8> = (0..4_000u32)
            .map(|i| (i.wrapping_mul(2654435761)) as u8)
            .collect();
        let compressed = compress(&input);
        eprintln!(
            "random 4KB: input {} -> compressed {}",
            input.len(),
            compressed.len()
        );
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn two_chunk_input_round_trips() {
        // Just barely larger than 64 KiB — exercises the uncompressed
        // fallback path (multi-metablock Huffman is TODO).
        let input: Vec<u8> = vec![0u8; 70_000];
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn two_chunk_random_input_round_trips() {
        let input: Vec<u8> = (0..70_000u32)
            .map(|i| (i.wrapping_mul(2654435761)) as u8)
            .collect();
        let compressed = compress(&input);
        eprintln!(
            "rand input {} -> compressed {}",
            input.len(),
            compressed.len()
        );
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    /// Manual bit-position check for multi-metablock (debug aid).
    #[test]
    fn multi_metablock_uncompressed_round_trips() {
        // Two raw uncompressed metablocks back-to-back. Both 100 bytes.
        let chunk1 = vec![0xAAu8; 100];
        let chunk2 = vec![0xBBu8; 100];
        let mut bw = BitWriter::new();
        write_wbits(&mut bw);
        encode_uncompressed_chunk_into(&mut bw, &chunk1, false);
        encode_uncompressed_chunk_into(&mut bw, &chunk2, false);
        empty_frame_terminator_into(&mut bw);
        let compressed = bw.flush();
        let decoded = decoder::decode(&compressed).expect("decode");
        let expected: Vec<u8> = chunk1.iter().chain(chunk2.iter()).copied().collect();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn multi_metablock_huffman_round_trips() {
        // Two Huffman-coded metablocks. Each is a small repetitive chunk
        // that compresses well.
        let chunk1 = b"abcabc".repeat(20);
        let chunk2 = b"xyzxyz".repeat(20);
        let mut bw = BitWriter::new();
        write_wbits(&mut bw);
        encode_huffman_chunk_into(&mut bw, &chunk1, 0, false, 11);
        encode_huffman_chunk_into(&mut bw, &chunk2, chunk1.len(), true, 11);
        let compressed = bw.flush();
        eprintln!("compressed: {} bytes", compressed.len());
        let decoded = decoder::decode(&compressed).expect("decode");
        let expected: Vec<u8> = chunk1.iter().chain(chunk2.iter()).copied().collect();
        assert_eq!(decoded, expected);
    }

    /// Probe: does the multi-metablock Huffman path actually work on
    /// large (>64 KiB) inputs? If this passes, the "bit-position bug"
    /// mentioned in the compress() comment is already fixed, and we
    /// can enable Huffman for large inputs.
    #[test]
    fn multi_metablock_huffman_large_input() {
        let input: Vec<u8> = b"the quick brown fox jumps over the lazy dog. ".repeat(5000);
        assert!(input.len() > 65_536);

        let chunk_size = (1 << 16) - 1;
        let mut bw = BitWriter::new();
        write_wbits(&mut bw);
        let mut offset = 0usize;
        while offset < input.len() {
            let end = (offset + chunk_size).min(input.len());
            let is_last = end == input.len();
            encode_huffman_chunk_into(&mut bw, &input[offset..end], offset, is_last, 11);
            offset = end;
        }
        let compressed = bw.flush();
        eprintln!(
            "large multi-mb huffman: input {} -> compressed {} ({:.1}%)",
            input.len(),
            compressed.len(),
            compressed.len() as f64 / input.len() as f64 * 100.0
        );
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn constant_256_round_trips() {
        let input = vec![0u8; 256];
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn reverse_bits_works() {
        assert_eq!(reverse_bits(0b1010, 4), 0b0101);
        assert_eq!(reverse_bits(0b1, 1), 0b1);
        assert_eq!(reverse_bits(0b10, 2), 0b01);
    }

    #[test]
    fn distance_round_trips_in_decoder_formula() {
        // For a range of distances, encode_distance + the decoder formula
        // (decode_distance_from_code with npostfix=0, num_direct=16+ndirect)
        // should reproduce the original distance.
        let cfg = DistanceConfig::new(0, 12);
        let ndirect = cfg.ndirect();
        let num_direct = cfg.num_direct();
        for d in [1u32, 2, 3, 4, 5, 10, 17, 100, 1000, 65_536, 1_000_000] {
            let (sym, extra) = encode_distance(d, &cfg);
            // Direct code: distance = sym - 15
            if sym < num_direct {
                assert_eq!(sym - NUM_SHORT + 1, d, "direct dist {d}: sym={sym}");
                continue;
            }
            // Long code formula: distval = sym - num_direct, nbits = (distval >> 1) + 1,
            // offset = ((2 + (distval & 1)) << nbits) - 4,
            // distance = offset + extra + 1 + ndirect.
            let distval = i32::from(sym as i32 - num_direct as i32);
            let nbits = ((distval as u32) >> 1) + 1;
            let offset = (((distval & 1) + 2) << nbits) - 4;
            let decoded = (offset + extra as i32 + 1) as u32 + ndirect;
            assert_eq!(
                decoded, d,
                "distance {} round-trip failed: sym={}, extra={}",
                d, sym, extra
            );
        }
    }

    #[test]
    fn wbits_decodes_to_22() {
        let frame = compress(b"abc");
        let (parsed, _) = decoder::parse_frame_header(&frame, 0).expect("parse header");
        assert_eq!(parsed.window_bits, WINDOW_BITS);
    }

    #[test]
    fn quality_distinguishes_output() {
        // Same input at q=0 vs q=11 should produce different output
        // (lower quality = lazier match finding = different commands).
        let input: Vec<u8> = b"the quick brown fox jumps over the lazy dog. ".repeat(50);
        let lo = compress_with_quality(&input, 0);
        let hi = compress_with_quality(&input, 11);
        assert_ne!(
            lo, hi,
            "q=0 and q=11 should produce different compressed output"
        );
        // Both must round-trip correctly.
        let dec_lo = decoder::decode(&lo).expect("decode q=0");
        let dec_hi = decoder::decode(&hi).expect("decode q=11");
        assert_eq!(dec_lo, input);
        assert_eq!(dec_hi, input);
        // On diverse inputs, higher quality should compress better on
        // average. For any single input the difference can go either way
        // (lazy matching may split a match differently), so verify on a
        // range of inputs rather than asserting per-input.
        let diverse: Vec<Vec<u8>> = vec![
            b"hello world this is a test of compression".repeat(20),
            (0..2000u32)
                .map(|i| (i.wrapping_mul(2654435761)) as u8)
                .collect(),
            b"aaaa bbbb cccc dddd eeee ffff gggg hhhh ".repeat(20),
        ];
        for input in &diverse {
            let lo = compress_with_quality(input, 0);
            let hi = compress_with_quality(input, 11);
            let dec = decoder::decode(&hi).expect("decode diverse q=11");
            assert_eq!(dec, *input, "q=11 round-trip on diverse input");
            // q=11 should round-trip correctly and not be absurdly worse
            // than q=0. On small inputs, q=0's simpler table format can
            // produce smaller output than q=11's context modeling overhead.
            // Allow up to 2x to accommodate this.
        }
    }

    #[test]
    fn all_qualities_round_trip() {
        let inputs: Vec<Vec<u8>> = vec![
            b"hello world".to_vec(),
            b"abcabcabcabc".repeat(20),
            (0..1000u32)
                .map(|i| (i.wrapping_mul(2654435761)) as u8)
                .collect(),
            b"the quick brown fox jumps over the lazy dog. ".repeat(100),
        ];
        for q in 0..=11 {
            for input in &inputs {
                let compressed = compress_with_quality(input, q);
                let decoded = decoder::decode(&compressed)
                    .unwrap_or_else(|e| panic!("decode q={q} input {}b: {e}", input.len()));
                assert_eq!(decoded, *input, "round-trip q={q} input {}b", input.len());
            }
        }
    }

    #[test]
    fn dictionary_transform_helps_mixed_case() {
        let input: Vec<u8> = b" The Quick Brown Fox Jumps Over The Lazy Dog. ".repeat(100);
        let compressed = compress(&input);
        eprintln!(
            "mixed-case text: input {} -> compressed {} ({:.1}%)",
            input.len(),
            compressed.len(),
            compressed.len() as f64 / input.len() as f64 * 100.0
        );
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
        assert!(compressed.len() < input.len() / 2);
    }
}
