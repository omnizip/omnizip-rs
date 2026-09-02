//! Metablock emission: turning a parsed command stream into brotli
//! bits (RFC 7932 §9-10). Extracted from [`crate::from_spec_encoder`]
//! — the parse/cost side stays there, everything that writes a
//! metablock's bits lives here.
//!
//! Byte-identical to the pre-extraction code by construction (pure
//! move; verified by fixture hashes at q1/q5/q9/q11 on code text,
//! CSV and FITS inputs).

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
use crate::encoder::distance_config::DistanceConfig;
#[cfg(test)]
use crate::from_spec_encoder::empty_frame_terminator_into;
use crate::from_spec_encoder::env_flag;
use crate::from_spec_encoder::{
    block_length_code, brotli_quality_config, build_rle_sequence, build_symbol_stream,
    canonical_with_reverse, decide_literal_contexts, distance_extra_bits, lit_split_forced_now,
    override_lengths_for_simple_form, parse_input_with_offset, reverse_bits,
    split_cmd_symbols_optimal, split_literals, split_symbol_stream_optimal,
    write_block_switch_header, write_context_map, write_simple_form_table, write_simple_one_symbol,
    write_varlen_uint8, zopfli_max_len, Command, RepBuffer, CL_CODE_TO_WIRE,
    CODE_LENGTH_CODE_ORDER, K_STATIC_CONTEXT_MAP_COMPLEX_UTF8, LIT0, MAX_BACKWARD_DISTANCE,
    MIN_MATCH, NTREES_COMPLEX_UTF8,
};
use crate::prefix::kCmdLut;

/// Append all bits from `src` (bytes + accumulator) to `dst`.
#[allow(dead_code)]
pub(crate) fn append_writer(dst: &mut BitWriter, src: BitWriter) {
    for byte in src.out {
        dst.write_bits(u32::from(byte), 8);
    }
    if src.nbits > 0 {
        dst.write_bits(src.acc as u32, src.nbits);
    }
}

/// Encode one metablock (Huffman-coded) into the shared writer.
pub(crate) fn encode_huffman_chunk_into(
    bw: &mut BitWriter,
    input: &[u8],
    mlen_offset: usize,
    is_last: bool,
    quality: i32,
    ctx_in: (u8, u8),
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
        hash_bytes: 4,
        max_match_length: zopfli_max_len(quality),
    };
    let mut mf = omnizip_codecs::HashChainMatchFinder::new(input, config);
    let hist_start = mlen_offset.min(MAX_BACKWARD_DISTANCE as usize);
    let _ = hist_start; // history unavailable on this path (single-chunk callers)
    encode_huffman_chunk_body(
        bw,
        input,
        &[],
        &mut mf,
        None,
        mlen_offset, // MF data[0] sits at global position mlen_offset
        mlen_offset,
        is_last,
        quality,
        ctx_in,
    );
}

/// Internal: encode one metablock with an external match finder.
/// The MF may reference the full input (cross-chunk) or just the
/// chunk slice (per-chunk), depending on the caller.
pub(crate) fn encode_huffman_chunk_body(
    bw: &mut BitWriter,
    input: &[u8],
    history: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    bank_mf: Option<&mut omnizip_codecs::BankMatchFinder>,
    mf_base: usize,
    mlen_offset: usize,
    is_last: bool,
    quality: i32,
    ctx_in: (u8, u8),
) {
    // Context modeling: at quality >= 4, split literals into context
    // trees. Active for Q4+ inputs ≥ 4 KiB (any content type — FSST-
    // transformed data benefits from context separation just as much
    // as natural text).
    // Context modeling earns its cost on TEXT (27% of CSV q5 output)
    // but only 1.8% on binary for 39% of encode time — binary q4-7
    // (the time-first tier) skips it. BROTLI_NO_CM forces off,
    // BROTLI_FORCE_CM forces on.
    let use_context = quality >= 4
        && input.len() >= 4096
        && !env_flag!("BROTLI_NO_CM")
        && (env_flag!("BROTLI_FORCE_CM") || quality >= 8 || is_text_like(input));

    // Block-type switching is disabled — testing showed a slight ratio
    // regression on uniform text data (per-block-type Huffman overhead
    // exceeds the benefit when statistics don't vary). The decoder now
    // correctly handles NBLTYPES > 1, and `write_block_type_trees` +
    // the inline switch emission in the literal loop are wired up, so
    // this can be flipped back on for inputs with strongly varying
    // per-block statistics.
    let use_block_switch = false;
    let phase_timer = env_flag!("BROTLI_PHASE_TIMER");
    let pt0 = std::time::Instant::now();
    let (commands, precomputed) = parse_input_with_offset(
        input,
        history,
        mf,
        bank_mf,
        mf_base,
        mlen_offset,
        quality,
        false,
        is_last,
        ctx_in,
    );
    if env_flag!("BROTLI_PARSE_AUDIT") {
        let mut pos = mlen_offset;
        for c in &commands {
            pos += c.insert_len as usize;
            if c.copy_len > 0 {
                let max_dist = (pos as u32).min(MAX_BACKWARD_DISTANCE);
                let advance = if c.distance > max_dist {
                    let mut scratch = Vec::new();
                    match crate::dictionary::dictionary_lookup(
                        &mut scratch,
                        c.copy_len,
                        c.distance as i32,
                        max_dist,
                    ) {
                        Some(()) => scratch.len(),
                        None => c.copy_len as usize,
                    }
                } else {
                    c.copy_len as usize
                };
                pos += advance;
            }
        }
        if pos != mlen_offset + input.len() {
            eprintln!(
                "PARSE-OVERRUN mlen_offset={mlen_offset} len={} accounted={pos}",
                input.len()
            );
            let mut p2 = mlen_offset;
            for c in &commands {
                p2 += c.insert_len as usize;
                if c.copy_len > 0 {
                    let max_dist = (p2 as u32).min(MAX_BACKWARD_DISTANCE);
                    let advance = if c.distance > max_dist {
                        let mut scratch = Vec::new();
                        match crate::dictionary::dictionary_lookup(
                            &mut scratch,
                            c.copy_len,
                            c.distance as i32,
                            max_dist,
                        ) {
                            Some(()) => scratch.len(),
                            None => c.copy_len as usize,
                        }
                    } else {
                        c.copy_len as usize
                    };
                    p2 += advance;
                    if p2 > mlen_offset + input.len() {
                        eprintln!(
                            "OFFENDING at~{p2} ins={} copy={} dist={} advance={advance}",
                            c.insert_len, c.copy_len, c.distance
                        );
                        break;
                    }
                }
            }
        }
    }
    // The exact-acceptance chain already emitted the winning parse
    // with these exact header parameters — reuse its bits verbatim
    // instead of a fourth full emission (3 measures + final became
    // 3 measures total at q10+). The winner writer contains the
    // metablock header too, so the header is only written on the
    // recompute path.
    if let Some(won) = precomputed {
        append_writer(bw, won);
    } else {
        bw.write_bits(u32::from(is_last), 1); // ISLAST
                                              // ISLASTEMPTY only present when ISLAST=1; we never emit empty
                                              // metablocks, so always 0 when present.
        if is_last {
            bw.write_bits(0, 1); // ISLASTEMPTY = 0
        }
        // MLEN encoding: pick smallest MNIBBLES that fits.
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
        // ISUNCOMPRESSED is only written when ISLAST=0 (matches
        // upstream `DecodeMetaBlockLength` gate).
        if !is_last {
            bw.write_bits(0, 1); // ISUNCOMPRESSED = 0
        }
        if phase_timer {
            eprintln!("PHASE parse={:.3}", pt0.elapsed().as_secs_f64());
        }
        let pt1 = std::time::Instant::now();
        emit_metablock_from_commands(
            bw,
            input,
            mlen_offset,
            is_last,
            quality,
            ctx_in,
            use_context,
            use_block_switch,
            &commands,
        );
        if phase_timer {
            eprintln!("PHASE emit={:.3}", pt1.elapsed().as_secs_f64());
        }
    }
}

/// Emission stage shared by the real encoder and parse-candidate
/// scoring: everything from the parsed command list to the last
/// tree-coded symbol. Pure with respect to `bw` — identical commands
/// produce identical bits.
#[allow(clippy::too_many_lines)]
pub(crate) fn emit_metablock_from_commands(
    bw: &mut BitWriter,
    input: &[u8],
    mlen_offset: usize,
    is_last: bool,
    quality: i32,
    ctx_in: (u8, u8),
    use_context: bool,
    use_block_switch: bool,
    commands: &[Command],
) {
    let _ = is_last;
    // Choose distance-code configuration from the parsed commands.
    let dist_cfg = DistanceConfig::choose(&commands, quality);

    let Some(stream) = build_symbol_stream(&commands, input, mlen_offset, &dist_cfg) else {
        // Header consistency: NBLTYPESI/D must still be written before
        // returning (the metablock prefix is already on the wire).
        write_varlen_uint8(bw, 0); // NBLTYPESI = 1
        write_varlen_uint8(bw, 0); // NBLTYPESD = 1
        return;
    };

    // --- Command block splitting (BrotliBuildMetaBlock cmd pass) ---
    // Splits buy ~2KB (0.07%) on binary q5 for ~18% of encode —
    // binary q4-7 skips them (text keeps them: they earn their cost).
    let cmd_split_on = quality >= 4
        && stream.cmd_symbols.len() >= 1024
        && !env_flag!("BROTLI_NO_SPLIT")
        && (quality >= 8 || is_text_like(input));
    // q10+ parses ride implicit-rep0 commands heavily; their symbol
    // stream is broader and keeps sharpening up to 64 blocks (measured
    // 16→64 saves ~3.9KB at 1MB q11; 128 regresses on switch overhead).
    // Below ~32K symbols the switch overhead wins — 100KB regressed.
    let max_blocks = std::env::var("BROTLI_SPLIT_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if quality >= 10 && stream.cmd_symbols.len() >= 32_768 {
            (stream.cmd_symbols.len() / 900).clamp(16, 64)
        } else {
            16
        });
    // Reference 1.2.0 command block split (SplitByteVector on the
    // command-symbol stream, 704-wide histograms). Default at q10+
    // (BROTLI_OLD_CSPLIT restores the in-house DP); the reference
    // emits ~5 cost-chosen blocks where the in-house K<=64 DP put 15+
    // on CSV q11, paying switch codes and tree headers.
    let ref_cmd_split =
        quality >= 10 && !env_flag!("BROTLI_OLD_CSPLIT") || env_flag!("BROTLI_REF_CSPLIT");
    let mut cmd_block_types: Vec<u8> = Vec::new();
    let cmd_boundaries: Vec<usize> = if !cmd_split_on {
        cmd_block_types.push(0);
        vec![0]
    } else if ref_cmd_split {
        let syms: Vec<u16> = stream.cmd_symbols.iter().map(|&s| s as u16).collect();
        let iters = if quality >= 11 { 10 } else { 3 };
        let split = crate::encoder::block_splitter::split_byte_vector(
            &syms,
            704,
            crate::encoder::block_splitter::SYMBOLS_PER_COMMAND_HISTOGRAM,
            crate::encoder::block_splitter::MAX_COMMAND_HISTOGRAMS,
            crate::encoder::block_splitter::COMMAND_STRIDE_LENGTH,
            crate::encoder::block_splitter::COMMAND_BLOCK_SWITCH_COST,
            iters,
        );
        let mut boundaries = vec![0usize];
        let mut pos = 0usize;
        for (&len, &ty) in split.lengths.iter().zip(split.types.iter()) {
            if len == 0 {
                continue;
            }
            pos += len as usize;
            cmd_block_types.push(ty);
            if pos < stream.cmd_symbols.len() {
                boundaries.push(pos);
            }
        }
        while cmd_block_types.len() > boundaries.len() {
            cmd_block_types.pop();
        }
        while cmd_block_types.len() < boundaries.len() {
            let next = cmd_block_types.len() as u8;
            cmd_block_types.push(next);
        }
        if env_flag!("BROTLI_DBG_CTX") {
            eprintln!(
                "CMDSPLIT ref blocks={} types={:?} n={}",
                boundaries.len(),
                &cmd_block_types,
                stream.cmd_symbols.len()
            );
        }
        boundaries
    } else {
        let b = split_cmd_symbols_optimal(&stream.cmd_symbols, max_blocks);
        cmd_block_types = (0..b.len() as u8).collect();
        b
    };
    let nbltypes_c: u32 = if cmd_boundaries.len() <= 1 {
        1
    } else {
        usize::from(cmd_block_types.iter().copied().max().unwrap_or(0)) as u32 + 1
    };
    let cmd_block_len: Vec<u32> = cmd_boundaries
        .iter()
        .enumerate()
        .map(|(k, &b)| {
            let end = cmd_boundaries
                .get(k + 1)
                .copied()
                .unwrap_or(stream.cmd_symbols.len());
            (end - b) as u32
        })
        .collect();
    // Per-command block-TYPE assignment (types, not ordinals — blocks
    // can reuse type ids).
    let cmd_block_of: Vec<u8> = {
        let mut a = vec![0u8; stream.cmd_symbols.len()];
        for (k, &b) in cmd_boundaries.iter().enumerate() {
            let end = cmd_boundaries
                .get(k + 1)
                .copied()
                .unwrap_or(stream.cmd_symbols.len());
            let ty = *cmd_block_types.get(k).unwrap_or(&(k as u8));
            for x in a.iter_mut().take(end).skip(b) {
                *x = ty;
            }
        }
        a
    };

    // --- Literal block splitting (BrotliBuildMetaBlock literal pass) ---
    // Below q10 the literal-tree assignment is the decided static map
    // (block-INdependent trees): literal block splits then only pay
    // switch-code overhead — a single literal block is both smaller
    // and faster. BROTLI_FORCE_LIT_SPLIT overrides; the q10/11 parse
    // contest sets the thread-local override to measure BOTH
    // assignments per candidate (either can win depending on the
    // corpus: the decided map wins on real CSV, the splitter wins by
    // ~25% of literal bits on strongly periodic text).
    let decided_early: Option<(usize, Vec<u8>)> =
        if quality >= 5 && use_context && is_text_like(input) && !lit_split_forced_now() {
            decide_literal_contexts(input, quality, mlen_offset + input.len())
        } else {
            None
        };
    let lit_split_on = quality >= 4
        && stream.literals.len() >= 4096
        && use_context
        && decided_early.is_none()
        && !env_flag!("BROTLI_NO_LIT_SPLIT");
    // Scale the block budget with the literal count: small inputs
    // lose more to block/tree overhead than they gain from sharper
    // local statistics (measured crossover near ~2K literals/block).
    let max_lit_blocks = std::env::var("BROTLI_LIT_SPLIT_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            // q10+ benefits from far finer literal blocks than the
            // literals/1024 scaling on large streams (measured: 96
            // blocks vs 39 at 1MB q11 saves 3.5KB); smaller streams
            // keep the conservative scaling.
            if quality >= 10 && stream.literals.len() >= 32_768 {
                (stream.literals.len() / 400).clamp(16, 96)
            } else {
                // q<10: cap keeps the (block x context) clustering's
                // second phase small (blocks/64 groups x 8 centroids);
                // a few hundred uncapped blocks measured 8s of
                // clustering for +1% size.
                (stream.literals.len() / 1024).clamp(8, 48)
            }
        });
    // Reference 1.2.0 literal block split (SplitByteVector port):
    // FindBlocks DP over sampled entropy codes + batched HistogramPair
    // clustering. Default at q10+ (BROTLI_OLD_LIT_SPLIT restores the
    // in-house DP); BROTLI_REF_LIT_SPLIT forces it at any level.
    // Default at q10+ (BROTLI_OLD_LIT_SPLIT restores the in-house DP).
    // The stream-corruption bug is fixed: reused block-type ids got
    // zero-frequency codes in the block-type tree (zero-length code =
    // no bits written on switch, desyncing the decoder); the bt
    // histogram now counts the actually emitted types.
    let ref_lit_split =
        quality >= 10 && !env_flag!("BROTLI_OLD_LIT_SPLIT") || env_flag!("BROTLI_REF_LIT_SPLIT");
    let mut lit_block_types: Vec<u8> = Vec::new();
    let lit_boundaries: Vec<usize> = if !lit_split_on {
        vec![]
    } else if ref_lit_split {
        let syms: Vec<u16> = stream.literals.iter().map(|&b| u16::from(b)).collect();
        let iters = if quality >= 11 { 10 } else { 3 };
        let split = crate::encoder::block_splitter::split_byte_vector(
            &syms,
            256,
            crate::encoder::block_splitter::SYMBOLS_PER_LITERAL_HISTOGRAM,
            crate::encoder::block_splitter::MAX_LITERAL_HISTOGRAMS,
            crate::encoder::block_splitter::LITERAL_STRIDE_LENGTH,
            crate::encoder::block_splitter::LITERAL_BLOCK_SWITCH_COST,
            iters,
        );
        // Boundaries from lengths; keep the per-boundary TYPE so the
        // context map stays type-major (blocks sharing a type must
        // share trees — decoder semantics).
        // Old boundary format: [0, cut1, ...] EXCLUDING the final
        // total (lit_block_len derives the last block via len).
        // Block i has split length Li and type Ti; the boundary list
        // is [0] + end positions of every block except the last, and
        // lit_block_types[i] is block i's type (cmap is type-major).
        let mut boundaries = vec![0usize];
        let mut pos = 0usize;
        for (i, (len, &ty)) in split.lengths.iter().zip(split.types.iter()).enumerate() {
            if *len == 0 {
                continue;
            }
            pos += *len as usize;
            lit_block_types.push(ty);
            if pos < stream.literals.len() {
                boundaries.push(pos);
            }
        }
        while lit_block_types.len() > boundaries.len() {
            lit_block_types.pop();
        }
        while lit_block_types.len() < boundaries.len() {
            let next = lit_block_types.len() as u8;
            lit_block_types.push(next);
        }
        if env_flag!("BROTLI_DBG_CTX") {
            eprintln!(
                "LITSPLIT ref blocks={} types={:?} boundaries={:?} litcount={}",
                boundaries.len(),
                lit_block_types,
                boundaries,
                stream.literals.len()
            );
        }
        boundaries
    } else {
        lit_block_types = (0..max_lit_blocks).map(|i| i as u8).collect();
        split_literals(&stream.literals, max_lit_blocks)
    };
    let nbltypes_l: u32 = if lit_boundaries.is_empty() {
        1
    } else {
        usize::from(lit_block_types.iter().copied().max().unwrap_or(0)) as u32 + 1
    };
    let _ = &max_lit_blocks;
    let lit_block_len: Vec<u32> = lit_boundaries
        .iter()
        .enumerate()
        .map(|(k, &b)| {
            let end = lit_boundaries
                .get(k + 1)
                .copied()
                .unwrap_or(stream.literals.len());
            (end - b) as u32
        })
        .collect();
    // Context mode selection: UTF8 (2) for text-like input, LSB6 (0) otherwise.
    // UTF8 gives better context separation for multi-byte chars and ASCII text.
    let context_mode: u32 = if use_context && is_text_like(input) {
        2 // UTF8
    } else if quality >= 10 {
        // Reference ChooseContextMode: q10+ non-UTF8 input gets the
        // SIGNED prior (sign-aware buckets of both previous bytes).
        3 // SIGNED
    } else {
        // Reference ChooseContextMode: UTF8 for ALL input below q10
        // (SIGNED is the q10+ branch). Measured vs our old LSB6 for
        // binary: fits q5 -161KB, arial q5 -45KB, bin1 q5/q9 -2.8KB.
        2 // UTF8
    };
    let mut decided_ctx: Option<(usize, Vec<u8>)> = decided_early.clone();
    let (mut ntrees_l, mut lit_ctx_map): (u32, Vec<u8>) = if use_block_switch {
        (2, (0..128u8).map(|i| i >> 6).collect())
    } else if use_context && context_mode == 2 && input.len() >= 1_048_576 && quality >= 10 {
        // Static complex UTF-8 context map (13 trees) for large text
        // inputs at Q10+. Ported from the reference encoder's
        // `kStaticContextMapComplexUTF64`.
        (
            NTREES_COMPLEX_UTF8,
            K_STATIC_CONTEXT_MAP_COMPLEX_UTF8.to_vec(),
        )
    } else if use_context && context_mode == 2 && quality >= 5 && !env_flag!("BROTLI_NO_CTX_DECIDE")
    {
        // Reference DecideOverLiteralContextModeling at q5-9: pick the
        // context map from sampled entropy instead of a fixed 4-tree
        // ctx>>4 split. The decided map participates in the A/B/C
        // assignment below as option C.
        let decided = decided_early
            .clone()
            .or_else(|| decide_literal_contexts(input, quality, mlen_offset + input.len()));
        if env_flag!("BROTLI_STATS") {
            eprintln!(
                "STATS ctx_decide q{quality}: {} contexts",
                decided.as_ref().map_or(1, |(n, _)| *n)
            );
        }
        decided_ctx = decided;
        match &decided_ctx {
            Some((n, map)) => (*n as u32, map.clone()),
            None => (1, Vec::new()),
        }
    } else if use_context && input.len() >= 8192 {
        (4u32, (0..64u8).map(|ctx| ctx >> 4).collect())
    } else if use_context {
        (2, (0..64u8).map(|ctx| u8::from(ctx >= 32)).collect())
    } else {
        (1, Vec::new())
    };

    let mut ntrees = ntrees_l as usize;
    let mut lit_freqs: Vec<Vec<u32>> = vec![vec![0u32; 256]; ntrees];

    // Precompute the actual copy advance for each command so subsequent
    // loops (frequency counting, encoding) advance correctly.
    //
    // Only the running output LENGTH is needed — the frequency loop
    // below reads the original input bytes (decoder output == input),
    // and every non-dictionary copy advances by exactly copy_len
    // (in-chunk, cross-chunk and the overrun zero-fill all add
    // copy_len bytes). Dictionary references are the one case where
    // the transformed length differs from cmd.copy_len (= word
    // length), so only they need the lookup. The previous revision
    // simulated the entire metablock byte-by-byte through a Vec to
    // derive these lengths (one push per literal + a per-byte copy
    // loop; measured at several % of q2 encode time).
    let mut cmd_copy_advances: Vec<usize> = Vec::with_capacity(commands.len());
    let mut dict_bytes: Vec<u8> = Vec::new();
    // Simulated output length so far == literals emitted + copy
    // advances (the position dictionary_lookup's is_dict check and
    // max_dist are computed against).
    let mut sim_len = 0usize;
    for cmd in commands {
        sim_len += cmd.insert_len as usize;
        let copy_advance = if cmd.copy_len > 0 {
            let copy_start_global = mlen_offset + sim_len;
            let max_dist = (copy_start_global as u32).min(MAX_BACKWARD_DISTANCE);
            // Use GLOBAL position for is_dict check. Cross-chunk LZ77
            // references have distance > local output length but ≤
            // global position. Using local position misidentifies them
            // as dict references, corrupting the advance accounting
            // and causing context ID mismatches between encoder and decoder.
            let is_dict =
                (cmd.distance as usize) > copy_start_global.min(MAX_BACKWARD_DISTANCE as usize);
            if is_dict {
                dict_bytes.clear();
                if dictionary_lookup(&mut dict_bytes, cmd.copy_len, cmd.distance as i32, max_dist)
                    .is_some()
                {
                    dict_bytes.len()
                } else {
                    cmd.copy_len as usize
                }
            } else {
                cmd.copy_len as usize
            }
        } else {
            0
        };
        sim_len += copy_advance;
        cmd_copy_advances.push(copy_advance);
    }

    // Compute per-tree frequencies. Since the decoder's output equals
    // the original input, we use input[out_pos] directly instead of
    // output_sim[out_pos]. This avoids corruption from cross-chunk LZ77
    // references that output_sim can't reproduce (it only has the
    // current chunk's data, not previous chunks').
    // p1/p2 CARRY across metablocks: upstream's context lookup reads the
    // frame ring buffer's last two bytes, so a continuation chunk's first
    // literals are contexted by the previous chunk's tail.
    let (mut p1, mut p2) = ctx_in;
    let mut out_pos = 0usize;
    let mut lit_block_type: usize = 0;
    let mut walk_assign: Vec<(usize, u8)> = Vec::new();
    // Per-(block, context) literal histograms. With literal block
    // splitting (nbltypes_l > 1), each block gets its own context→tree
    // mapping; trees are shared across blocks (NTREES_L total).
    let bc_hists: Vec<[u32; 256]> = vec![[0u32; 256]; nbltypes_l as usize * 64];
    let max_lit_blocks_dbg = lit_boundaries.len();
    let nbltypes_l_dbg = nbltypes_l;
    let mut bc_hists = bc_hists;
    {
        let mut lit_pos = 0usize;
        let mut lit_blk = 0usize;
        let mut next_b = 1usize;
        for (cmd_idx, cmd) in commands.iter().enumerate() {
            for _ in 0..cmd.insert_len {
                if nbltypes_l > 1
                    && lit_blk + 1 < lit_boundaries.len()
                    && lit_pos >= lit_boundaries[lit_blk + 1]
                {
                    lit_blk += 1;
                    next_b += 1;
                }
                let _ = next_b;
                let b = input[out_pos];
                let ctx_id = compute_context_id(p1, p2, context_mode) as usize;
                let blk_ty = *lit_block_types.get(lit_blk).unwrap_or(&(lit_blk as u8)) as usize;
                bc_hists[(blk_ty << 6) + ctx_id][b as usize] += 1;
                if env_flag!("BROTLI_WALK_TRACE") {
                    walk_assign.push(((blk_ty << 6) + ctx_id, b));
                }
                p2 = p1;
                p1 = b;
                out_pos += 1;
                lit_pos += 1;
            }
            if cmd.copy_len > 0 {
                out_pos += cmd_copy_advances[cmd_idx];
                if out_pos > 0 && out_pos <= input.len() {
                    // Mirror the decoder exactly: for copies ≥ 2 bytes,
                    // p2 = second-to-last copied byte (NOT the pre-copy p1).
                    // A wrong p2 selects a different literal context tree
                    // than the decoder on fine-grained context maps.
                    let new_p1 = input[out_pos - 1];
                    p2 = if cmd.copy_len > 1 {
                        input[out_pos - 2]
                    } else {
                        p1
                    };
                    p1 = new_p1;
                }
            }
        }
    }

    // Data-driven tree assignment: isolate pure/low-diversity contexts
    // into dedicated (often single-symbol, zero-bit) trees; cluster the
    // rest into shared trees. Replaces the static map whenever
    // per-(block,context) histograms are available.
    if env_flag!("BROTLI_DBG_CTX") {
        let mut rows: Vec<(usize, u64, usize)> = bc_hists
            .iter()
            .enumerate()
            .map(|(i, h)| {
                (
                    i,
                    h.iter().map(|&x| u64::from(x)).sum(),
                    h.iter().filter(|&&x| x > 0).count(),
                )
            })
            .collect();
        rows.sort_by_key(|&(_, c, _)| std::cmp::Reverse(c));
        for (i, c, d) in rows.iter().take(20) {
            eprintln!("CTXDBG bucket[{i}] count={c} distinct={d}");
        }
    }
    {
        // Compare two tree strategies by expected wire cost and keep
        // the cheaper: (A) plain clustering vs (B) singleton isolation.
        // On literal-sparse inputs B's tree+cmap overhead exceeds its
        // zero-bit-literal savings.
        let tree_bits = |h: &[u32; 256]| -> f64 {
            let t: u64 = h.iter().map(|&x| u64::from(x)).sum();
            if t == 0 {
                return 0.0;
            }
            let mut e = 0.0f64;
            for &f in h.iter() {
                if f > 0 {
                    let p = f as f64 / t as f64;
                    e -= f as f64 * p.log2();
                }
            }
            e
        };
        // The A/B clustering passes cost ~20% of q5-9 encode; at those
        // qualities the reference uses ONLY its decided static map.
        // Take C directly there (BROTLI_FULL_ASSIGN restores the A/B/C
        // comparison); q10+ keeps the full machinery.
        let skip_ab = quality < 10 && decided_ctx.is_some() && !env_flag!("BROTLI_FULL_ASSIGN");
        // Literal-tree clustering cap: the reference's ContextBlockSplitter
        // reaches >100 trees at q11 (FITS: 143); a cap of 4 forfeits ~360KB
        // of literal entropy there. BROTLI_LIT_TREES overrides.
        let lit_trees_cap: usize = std::env::var("BROTLI_LIT_TREES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(if quality >= 10 { 64 } else { 4 });
        let cmap_a = if skip_ab {
            Vec::new()
        } else if env_flag!("BROTLI_STATS") {
            let t = std::time::Instant::now();
            let r = crate::encoder::context::cluster_contexts(&bc_hists, lit_trees_cap);
            eprintln!(
                "STATS cluster_lit rows={} blocks_dbg={max_lit_blocks_dbg} nbltypes_l={} cap={max_lit_blocks} n_literals={} {:.2}s",
                bc_hists.len(),
                nbltypes_l_dbg,
                stream.literals.len(),
                t.elapsed().as_secs_f64()
            );
            r
        } else {
            crate::encoder::context::cluster_contexts(&bc_hists, lit_trees_cap)
        };
        let mut hists_a: Vec<[u32; 256]> = vec![[0u32; 256]; lit_trees_cap];
        let (cost_a, cmap_b, count_b, cost_b);
        if skip_ab {
            cost_a = f64::INFINITY;
            cmap_b = Vec::new();
            count_b = 0;
            cost_b = f64::INFINITY;
        } else {
            for (i, h) in bc_hists.iter().enumerate() {
                for (b, &f) in h.iter().enumerate() {
                    hists_a[usize::from(cmap_a[i])][b] += f;
                }
            }
            cost_a = hists_a.iter().map(|h| tree_bits(h)).sum::<f64>()
                + 4.0 * 60.0
                + bc_hists.len() as f64 * 2.0;
            let (cmap_b_v, count_b_v) =
                crate::encoder::context::assign_context_trees(&bc_hists, ntrees.max(4));
            cmap_b = cmap_b_v;
            count_b = count_b_v;
            let mut hists_b: Vec<[u32; 256]> = vec![[0u32; 256]; count_b];
            for (i, h) in bc_hists.iter().enumerate() {
                for (b, &f) in h.iter().enumerate() {
                    hists_b[usize::from(cmap_b[i])][b] += f;
                }
            }
            cost_b = hists_b.iter().map(|h| tree_bits(h)).sum::<f64>()
                + count_b as f64 * 35.0
                + bc_hists.len() as f64 * (count_b as f64).log2().max(1.0);
        }
        // Option C: the reference's decided static map (tree by ctx
        // only, independent of block).
        let mut cost_c = f64::INFINITY;
        let mut cmap_c: Vec<u8> = Vec::new();
        let mut count_c = 0usize;
        if let Some((n, map)) = &decided_ctx {
            let mut hists_c: Vec<[u32; 256]> = vec![[0u32; 256]; *n];
            for (i, h) in bc_hists.iter().enumerate() {
                let t = usize::from(map[i & 63]);
                for (b, &f) in h.iter().enumerate() {
                    hists_c[t][b] += f;
                }
            }
            count_c = hists_c.iter().filter(|h| h.iter().sum::<u32>() > 0).count();
            if count_c > 0 {
                cost_c = hists_c.iter().map(|h| tree_bits(h)).sum::<f64>()
                    + count_c as f64 * 60.0
                    + bc_hists.len() as f64 * (count_c as f64).log2().max(1.0);
                // Compact tree ids: empty static-map trees are dropped.
                let mut c_remap = vec![usize::MAX; *n];
                let mut next = 0usize;
                for (t, h) in hists_c.iter().enumerate() {
                    if h.iter().sum::<u32>() > 0 {
                        c_remap[t] = next;
                        next += 1;
                    }
                }
                cmap_c = (0..bc_hists.len())
                    .map(|i| {
                        let t = c_remap[usize::from(map[i & 63])];
                        u8::try_from(t.min(count_c - 1)).unwrap_or(0)
                    })
                    .collect();
            }
        }
        // Option R: reference 1.2.0 histogram clustering (cluster_inc.h
        // port) over the per-(block,context) histograms — the machinery
        // behind the reference's 143-tree literal modeling at q11.
        // Opt-in (BROTLI_REF_CLUST): on current fixtures it TIES option
        // A within 0.5% (the reference's edge comes from its literal
        // BLOCK SPLITTING, not finer context clustering) while costing
        // ~3s on FITS q11.
        let mut cost_r = f64::INFINITY;
        let mut cmap_r: Vec<u8> = Vec::new();
        let mut count_r = 0usize;
        if quality >= 10 || env_flag!("BROTLI_REF_CLUST") {
            let hists: Vec<crate::encoder::block_splitter::Hist> = bc_hists
                .iter()
                .map(|h| {
                    let mut x = crate::encoder::block_splitter::Hist::new(256);
                    x.data.copy_from_slice(h);
                    x.total = h.iter().map(|&v| u64::from(v)).sum();
                    x
                })
                .collect();
            let (trees, symbols) = crate::encoder::block_splitter::cluster_histograms(&hists, 256);
            count_r = trees.len();
            if count_r > 0 {
                let hists_r: Vec<[u32; 256]> = trees
                    .iter()
                    .map(|t| {
                        let mut a = [0u32; 256];
                        a.copy_from_slice(&t.data);
                        a
                    })
                    .collect();
                cost_r = hists_r.iter().map(|h| tree_bits(h)).sum::<f64>()
                    + count_r as f64 * 60.0
                    + bc_hists.len() as f64 * (count_r as f64).log2().max(1.0);
                cmap_r = symbols.iter().map(|&sy| sy as u8).collect();
            }
        }
        let (cmap, tree_count) = if cost_r < cost_a && cost_r < cost_b && cost_r < cost_c {
            (cmap_r, count_r)
        } else if cost_c < cost_a && cost_c < cost_b {
            (cmap_c, count_c)
        } else if cost_b < cost_a && !env_flag!("BROTLI_NO_SINGLETONS") {
            (cmap_b, count_b)
        } else {
            (
                cmap_a.clone(),
                cmap_a
                    .iter()
                    .copied()
                    .max()
                    .map_or(1, |m| usize::from(m) + 1),
            )
        };
        if env_flag!("BROTLI_DBG_CTX") {
            eprintln!(
                "ASSIGN cost_a={cost_a:.0} cost_b={cost_b:.0} cost_c={cost_c:.0} cost_r={cost_r:.0} trees={tree_count}"
            );
        }
        lit_ctx_map.clear();
        lit_ctx_map.extend_from_slice(&cmap);
        ntrees_l = tree_count as u32;
        ntrees = tree_count;
        lit_freqs = vec![vec![0u32; 256]; ntrees];
    }
    if env_flag!("BROTLI_DBG_CTX") {
        let mx = lit_ctx_map.iter().max().copied().unwrap_or(0);
        eprintln!(
            "CTXMAP len={} ntrees={ntrees} max_val={mx} decided={}",
            lit_ctx_map.len(),
            decided_ctx.as_ref().map_or(0, |(n, _)| *n)
        );
    }
    for (cm_idx, hist) in bc_hists.iter().enumerate() {
        let tree = if ntrees > 1 {
            lit_ctx_map[cm_idx] as usize
        } else {
            0
        };
        for (b, &f) in hist.iter().enumerate() {
            lit_freqs[tree][b] += f;
        }
    }
    // Prune unused trees: the assignment may create tree ids that no
    // literal lands in (rare contexts, empty clusters). Every unused
    // tree would still cost a full header, so compact the ids and
    // remap the context map.
    if ntrees > 1 {
        let mut remap = vec![usize::MAX; ntrees];
        let mut next = 0usize;
        for t in 0..ntrees {
            let total: u32 = lit_freqs[t].iter().sum();
            if total > 0 {
                remap[t] = next;
                next += 1;
            }
        }
        if next == 0 {
            remap[0] = 0;
            next = 1;
        }
        let compact: Vec<Vec<u32>> = (0..ntrees)
            .filter(|&t| remap[t] != usize::MAX)
            .map(|t| std::mem::take(&mut lit_freqs[t]))
            .collect();
        lit_freqs = compact;
        for e in lit_ctx_map.iter_mut() {
            *e = remap[usize::from(*e)].min(next - 1) as u8;
        }
        ntrees = next;
        ntrees_l = next as u32;
    }

    write_varlen_uint8(bw, nbltypes_l - 1); // NBLTYPESL
    let mut lit_bt_wire: Vec<(u32, u8)> = Vec::new();
    let mut lit_bl_wire: Vec<(u32, u8)> = Vec::new();
    if nbltypes_l > 1 {
        let lit_switch_types: Vec<u8> = lit_block_types
            .iter()
            .enumerate()
            .filter(|(i, _)| *i > 0)
            .map(|(_, &t)| t)
            .collect();
        let (bt, bl) = write_block_switch_header(bw, nbltypes_l, &lit_block_len, &lit_switch_types);
        lit_bt_wire = bt;
        lit_bl_wire = bl;
    }
    write_varlen_uint8(bw, nbltypes_c - 1); // NBLTYPESI
    let mut cmd_bt_wire: Vec<(u32, u8)> = Vec::new();
    let mut cmd_bl_wire: Vec<(u32, u8)> = Vec::new();
    if nbltypes_c > 1 {
        let cmd_switch_types: Vec<u8> = cmd_block_types
            .iter()
            .enumerate()
            .filter(|(i, _)| *i > 0)
            .map(|(_, &t)| t)
            .collect();
        let (bt, bl) = write_block_switch_header(bw, nbltypes_c, &cmd_block_len, &cmd_switch_types);
        cmd_bt_wire = bt;
        cmd_bl_wire = bl;
    }
    // --- Distance block splitting: NBLTYPES_D > 1 with per-block-type
    // context maps (before NPOSTFIX per the wire order). ---
    let dist_split_on = quality >= 4
        && stream.dist_symbols.len() >= 1024
        && dist_cfg.alphabet_size() <= 256
        && !env_flag!("BROTLI_NO_DSPLIT")
        && (quality >= 8 || is_text_like(input));
    // Reference 1.2.0 distance block split (SplitByteVector on the
    // distance-symbol stream). The in-house DP capped blocks at 4 and
    // trees at 4; the reference clusters its histograms freely and
    // routinely emits 10-20 distance trees on text (ours was stuck at
    // 4, costing ~30K bits of dist_sym entropy on the CSV fixture).
    // Default at q10+ (BROTLI_OLD_DSPLIT restores the in-house DP);
    // BROTLI_REF_DSPLIT forces it at any level.
    let ref_dist_split =
        quality >= 5 && !env_flag!("BROTLI_OLD_DSPLIT") || env_flag!("BROTLI_REF_DSPLIT");
    let mut dist_block_types: Vec<u8> = Vec::new();
    let dist_boundaries: Vec<usize> = if !dist_split_on {
        dist_block_types.push(0);
        vec![0]
    } else if ref_dist_split {
        let syms: Vec<u16> = stream.dist_symbols.iter().map(|&s| s as u16).collect();
        let iters = if quality >= 11 { 10 } else { 3 };
        let split = crate::encoder::block_splitter::split_byte_vector(
            &syms,
            256,
            crate::encoder::block_splitter::SYMBOLS_PER_DISTANCE_HISTOGRAM,
            crate::encoder::block_splitter::MAX_COMMAND_HISTOGRAMS,
            crate::encoder::block_splitter::DISTANCE_STRIDE_LENGTH,
            crate::encoder::block_splitter::DISTANCE_BLOCK_SWITCH_COST,
            iters,
        );
        // Boundaries from lengths; keep the per-block TYPE (blocks
        // sharing a type share trees — the cmap is type-major).
        let mut boundaries = vec![0usize];
        let mut pos = 0usize;
        for (&len, &ty) in split.lengths.iter().zip(split.types.iter()) {
            if len == 0 {
                continue;
            }
            pos += len as usize;
            dist_block_types.push(ty);
            if pos < stream.dist_symbols.len() {
                boundaries.push(pos);
            }
        }
        while dist_block_types.len() > boundaries.len() {
            dist_block_types.pop();
        }
        while dist_block_types.len() < boundaries.len() {
            let next = dist_block_types.len() as u8;
            dist_block_types.push(next);
        }
        if env_flag!("BROTLI_DBG_CTX") {
            eprintln!(
                "DISTSPLIT ref blocks={} types={:?} n={}",
                boundaries.len(),
                &dist_block_types,
                stream.dist_symbols.len()
            );
        }
        boundaries
    } else {
        let b = split_symbol_stream_optimal(
            &stream
                .dist_symbols
                .iter()
                .map(|&s| s as usize)
                .collect::<Vec<_>>(),
            dist_cfg.alphabet_size(),
            4,
        );
        dist_block_types = (0..b.len() as u8).collect();
        b
    };
    // NBLTYPESD counts DISTINCT type ids, not blocks (the reference
    // ClusterBlocks reuses ids across non-adjacent blocks).
    let nbltypes_d: u32 = if dist_boundaries.len() <= 1 {
        1
    } else {
        usize::from(dist_block_types.iter().copied().max().unwrap_or(0)) as u32 + 1
    };
    let dist_block_len: Vec<u32> = dist_boundaries
        .iter()
        .enumerate()
        .map(|(k, &b)| {
            let end = dist_boundaries
                .get(k + 1)
                .copied()
                .unwrap_or(stream.dist_symbols.len());
            (end - b) as u32
        })
        .collect();
    write_varlen_uint8(bw, nbltypes_d - 1); // NBLTYPESD
    let mut dist_bt_wire: Vec<(u32, u8)> = Vec::new();
    let mut dist_bl_wire: Vec<(u32, u8)> = Vec::new();
    if nbltypes_d > 1 {
        let dist_switch_types: Vec<u8> = dist_block_types
            .iter()
            .enumerate()
            .filter(|(i, _)| *i > 0)
            .map(|(_, &t)| t)
            .collect();
        let (bt, bl) =
            write_block_switch_header(bw, nbltypes_d, &dist_block_len, &dist_switch_types);
        dist_bt_wire = bt;
        dist_bl_wire = bl;
    }

    bw.write_bits(dist_cfg.npostfix as u32, 2); // NPOSTFIX
    bw.write_bits(dist_cfg.ndirect_code as u32, 4); // NDMOEM

    // Context mode fields: one PER literal block type (RFC 7932 §9.3).
    for _ in 0..nbltypes_l {
        bw.write_bits(context_mode, 2);
    }

    write_varlen_uint8(bw, ntrees_l - 1); // NTREESL
    if ntrees_l > 1 {
        write_context_map(bw, &lit_ctx_map, ntrees_l);
    }
    // Distance context modeling (RFC 7932 §9.6): NTREES_D = 2 with the
    // context derived from copy length (kCmdLut.context = (len>4)?3:len-2).
    // Short copies ride a short-code-heavy tree; long copies a long-code
    // tree — each sharper than the blended single tree.
    let mut ntrees_d: u32 =
        if quality >= 4 && !stream.dist_symbols.is_empty() && !env_flag!("BROTLI_NO_DTREES") {
            std::env::var("BROTLI_DTREES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4)
        } else {
            1
        };
    // Distance context trees over per-(block, context) buckets, with
    // unused trees pruned and a cost gate against the single-tree
    // variant. Written after the literal context map per wire order.
    let dist_alphabet = dist_cfg.alphabet_size();
    let nb_d = nbltypes_d as usize;
    // Per-(block, context) histograms only when symbols fit the fixed
    // 256-wide buckets (NPOSTFIX > 0 alphabets can reach 520).
    let dist_bc_ok =
        dist_alphabet <= 256 && stream.dist_symbols.iter().all(|&s| (s as usize) < 256);
    let mut dist_bc_hists: Vec<[u32; 256]> = if dist_bc_ok {
        vec![[0u32; 256]; nb_d * 4]
    } else {
        vec![[0u32; 256]; 1]
    };
    if dist_bc_ok {
        let mut blk = 0usize;
        for (idx, (&sym, &ctx)) in stream
            .dist_symbols
            .iter()
            .zip(stream.dist_ctxs.iter())
            .enumerate()
        {
            while blk + 1 < dist_boundaries.len() && idx >= dist_boundaries[blk + 1] {
                blk += 1;
            }
            let ty = *dist_block_types.get(blk).unwrap_or(&(blk as u8)) as usize;
            dist_bc_hists[(ty << 2) + ctx as usize][sym as usize] += 1;
        }
    }
    let ent = |h: &[u32; 256]| -> f64 {
        let t: u64 = h.iter().map(|&x| u64::from(x)).sum();
        if t == 0 {
            return 0.0;
        }
        let mut e = 0.0f64;
        for &v in h.iter() {
            if v > 0 {
                e -= v as f64 * (v as f64 / t as f64).log2();
            }
        }
        e
    };
    let mut global_hist = [0u32; 256];
    if dist_bc_ok {
        for h in &dist_bc_hists {
            for (s, &v) in h.iter().enumerate() {
                global_hist[s] += v;
            }
        }
    }
    // Cost gates: (A) single tree, (B) per-(block,ctx) clustered trees.
    let global_hist = if dist_bc_ok {
        global_hist
    } else {
        let mut g = [0u32; 256];
        for &s in &stream.dist_symbols {
            if (s as usize) < 256 {
                g[s as usize] += 1;
            }
        }
        g
    };
    let cost_a = ent(&global_hist) + 70.0 + if dist_bc_ok { 0.0 } else { 1.0e9 };
    // Reference split clusters freely; the in-house DP's 4-tree cap
    // was tuned to its 4 blocks. Scaled tree counts pay off from q5
    // on code-like text (rustsrc q9 -1.2KB, FITS q9 -5.9KB, csv-real
    // q5/q9 -0.5/-0.7KB; 100KB CI fixtures neutral).
    let shared_k = if ref_dist_split {
        (nb_d * 4).clamp(2, 32)
    } else {
        ntrees_d.min(4) as usize
    };
    let cmap_bc = if env_flag!("BROTLI_STATS") {
        let t = std::time::Instant::now();
        let r = crate::encoder::context::cluster_contexts(&dist_bc_hists, shared_k);
        eprintln!(
            "STATS cluster_dist rows={} {:.2}s",
            dist_bc_hists.len(),
            t.elapsed().as_secs_f64()
        );
        r
    } else if ref_dist_split && quality >= 10 {
        // Cost-driven clustering (the reference ClusterHistograms PQ
        // combiner) picks the tree count by PopulationCost instead of a
        // fixed k — the greedy k=32 produced 31 trees where the
        // reference emits ~8, paying the difference back in headers.
        let mut hists: Vec<crate::encoder::block_splitter::Hist> = dist_bc_hists
            .iter()
            .map(|h| {
                let mut x = crate::encoder::block_splitter::Hist::new(256);
                x.data.copy_from_slice(h);
                x.total = h.iter().map(|&v| u64::from(v)).sum();
                x.recompute_cost();
                x
            })
            .collect();
        let (_merged, assign) =
            crate::encoder::block_splitter::cluster_histograms(&hists, shared_k);
        assign.iter().map(|&a| a as u8).collect()
    } else {
        crate::encoder::context::cluster_contexts(&dist_bc_hists, shared_k)
    };
    let used_count = {
        let mut hists: Vec<[u32; 256]> = vec![[0u32; 256]; shared_k];
        for (i, h) in dist_bc_hists.iter().enumerate() {
            for (s, &v) in h.iter().enumerate() {
                hists[usize::from(cmap_bc[i])][s] += v;
            }
        }
        hists.iter().filter(|h| h.iter().sum::<u32>() > 0).count()
    };
    let cost_b: f64 = {
        let mut hists: Vec<[u32; 256]> = vec![[0u32; 256]; shared_k];
        for (i, h) in dist_bc_hists.iter().enumerate() {
            for (s, &v) in h.iter().enumerate() {
                if v > 0 {
                    hists[cmap_bc[i] as usize][s] += v;
                }
            }
        }
        hists.iter().map(|h| ent(h)).sum::<f64>()
            + used_count as f64 * 70.0
            + nb_d as f64 * 4.0 * 2.0
    };
    let mut dist_freqs_per_ctx: Vec<Vec<u32>>;
    let mut ntrees_d_out: u32;
    let mut dist_cmap_full: Vec<u8>;
    if env_flag!("BROTLI_DBG_DCLUST") {
        eprintln!(
            "DCLUST cost_a={cost_a:.1} cost_b={cost_b:.1} used={used_count} rows={} k={shared_k} nb_d={nb_d} ref={ref_dist_split}",
            dist_bc_hists.len()
        );
    }
    if cost_b < cost_a {
        dist_freqs_per_ctx = vec![vec![0u32; dist_alphabet]; shared_k];
        for (i, h) in dist_bc_hists.iter().enumerate() {
            for (s, &v) in h.iter().enumerate() {
                if v > 0 && s < dist_alphabet {
                    dist_freqs_per_ctx[cmap_bc[i] as usize][s] += v;
                }
            }
        }
        dist_cmap_full = cmap_bc;
        ntrees_d_out = shared_k as u32;
        // Prune unused trees.
        let used: Vec<bool> = dist_freqs_per_ctx
            .iter()
            .map(|f| f.iter().sum::<u32>() > 0)
            .collect();
        let mut remap = vec![0usize; shared_k];
        let mut next = 0usize;
        for (t, u) in used.iter().enumerate() {
            if *u {
                remap[t] = next;
                next += 1;
            }
        }
        if next == 0 {
            next = 1;
        }
        dist_freqs_per_ctx = (0..shared_k)
            .filter(|&t| used[t])
            .map(|t| std::mem::take(&mut dist_freqs_per_ctx[t]))
            .collect();
        for e in dist_cmap_full.iter_mut() {
            *e = remap[usize::from(*e)].min(next - 1) as u8;
        }
        ntrees_d_out = next as u32;
    } else {
        dist_freqs_per_ctx = vec![global_hist.to_vec()];
        dist_cmap_full = vec![0u8; nb_d * 4];
        ntrees_d_out = 1;
    }
    write_varlen_uint8(bw, ntrees_d_out - 1); // NTREESD
    if ntrees_d_out > 1 {
        write_context_map(bw, &dist_cmap_full, ntrees_d_out);
    }
    ntrees_d = ntrees_d_out;
    let dist_ctx_tree_of = |blk: usize, ctx: u8| -> usize {
        let ty = *dist_block_types.get(blk).unwrap_or(&(blk as u8)) as usize;
        usize::from(dist_cmap_full[(ty << 2) + ctx as usize])
    };

    // --- Context modeling: per-tree literal frequencies ---
    // For NTREES_L > 1, partition literals by their LSB6 context.
    // Build a virtual output buffer to correctly track the "previous byte"
    // for context computation (copies change the previous byte too).
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

    let mut cmd_freqs_per_block: Vec<Vec<u32>> = vec![vec![0u32; 704]; nbltypes_c as usize];
    for (i, &sym) in stream.cmd_symbols.iter().enumerate() {
        cmd_freqs_per_block[usize::from(cmd_block_of[i])][sym as usize] += 1;
    }
    if let Ok(path) = std::env::var("BROTLI_DUMP_CMDSYM") {
        use std::io::Write as _;
        let mut f = std::fs::File::create(&path).unwrap();
        for &sym in &stream.cmd_symbols {
            writeln!(f, "{sym}").unwrap();
        }
    }
    for freq in &mut cmd_freqs_per_block {
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
    // (Per-context distance frequencies were computed before the
    // NTREESD header write — see dist_freqs_per_ctx / dist_ctx_tree.)

    // Dump per-tree literal frequencies for isolated round-trip tests.
    if env_flag!("BROTLI_DUMP_TREES") {
        for (i, freq) in lit_freqs.iter().enumerate() {
            let total: u32 = freq.iter().sum();
            let nz = freq.iter().filter(|&&f| f > 0).count();
            eprintln!("TREE {i} ntrees={ntrees} total={total} nz={nz} freqs={freq:?}");
        }
    }

    // Build per-tree literal Huffman tables.
    let mut lit_lengths_per_tree: Vec<omnizip_codecs::HuffmanLengths> = lit_freqs
        .iter()
        .map(|freq| omnizip_codecs::HuffmanLengths::build(freq, 15))
        .collect();
    let cmd_lengths_per_block: Vec<omnizip_codecs::HuffmanLengths> = cmd_freqs_per_block
        .iter()
        .map(|freq| omnizip_codecs::HuffmanLengths::build(freq, 15))
        .collect();
    let cmd_lengths = omnizip_codecs::HuffmanLengths::build(&cmd_freq, 15);
    let dist_lengths_per_ctx: Vec<omnizip_codecs::HuffmanLengths> = if ntrees_d > 1 {
        dist_freqs_per_ctx
            .iter()
            .map(|freq| omnizip_codecs::HuffmanLengths::build(freq, 15))
            .collect()
    } else {
        vec![omnizip_codecs::HuffmanLengths::build(&dist_freq, 15)]
    };
    let dist_lengths = omnizip_codecs::HuffmanLengths::build(&dist_freq, 15);

    // Diagnostic: entropy breakdown of the final symbol streams.
    if env_flag!("BROTLI_STATS") {
        let lit_bits: u64 = lit_freqs
            .iter()
            .zip(lit_lengths_per_tree.iter())
            .map(|(freq, huff)| {
                freq.iter()
                    .zip(huff.lengths.iter())
                    .map(|(&f, &l)| u64::from(f) * u64::from(l))
                    .sum::<u64>()
            })
            .sum();
        let cmd_sym_bits: u64 = cmd_freq
            .iter()
            .zip(cmd_lengths.lengths.iter())
            .map(|(&f, &l)| u64::from(f) * u64::from(l))
            .sum();
        // insert/copy extra bits from kCmdLut
        let cmd_extra_bits: u64 = stream
            .cmd_symbols
            .iter()
            .map(|&sym| {
                let e = &kCmdLut[sym as usize];
                u64::from(e.insert_len_extra_bits) + u64::from(e.copy_len_extra_bits)
            })
            .sum();
        let dist_sym_bits: u64 = dist_freq
            .iter()
            .zip(dist_lengths.lengths.iter())
            .map(|(&f, &l)| u64::from(f) * u64::from(l))
            .sum();
        let dist_extra_bits: u64 = {
            // extra bits depend on the distance config; recompute per symbol
            let mut total = 0u64;
            for &sym in &stream.dist_symbols {
                total += u64::from(distance_extra_bits(sym, &dist_cfg));
            }
            total
        };
        let n_rep = stream.dist_symbols.iter().filter(|&&s| s < 4).count();
        // Top distance VALUES (decoded from symbols + extras).
        let mut dist_values: std::collections::BTreeMap<u32, u32> =
            std::collections::BTreeMap::new();
        {
            let mut rep = RepBuffer::new();
            let mut out_pos = 0usize;
            let mut di = stream.dist_symbols.iter();
            for cmd in commands {
                out_pos += cmd.insert_len as usize;
                if cmd.copy_len > 0 {
                    let is_dict = (cmd.distance as usize)
                        > (mlen_offset + out_pos).min(MAX_BACKWARD_DISTANCE as usize);
                    if is_dict {
                        rep.on_dict_reference(false);
                    } else if rep.find_rep_code(cmd.distance).is_some() {
                        // rep: distance value already counted via cmd
                    }
                    *dist_values.entry(cmd.distance).or_insert(0) += 1;
                    if rep.find_rep_code(cmd.distance).is_some() {
                        // update below via find again
                    }
                    match rep.find_rep_code(cmd.distance) {
                        Some(code) => rep.on_rep_lz77(code),
                        None => rep.on_new_distance_lz77(cmd.distance),
                    }
                    out_pos += cmd.copy_len as usize;
                }
            }
            let _ = di;
        }
        let mut top: Vec<(u32, u32)> = dist_values.into_iter().collect();
        top.sort_by_key(|&(_d, c)| std::cmp::Reverse(c));
        top.truncate(8);
        eprintln!(
            "STATS ntrees={ntrees}: cmds={} literals={} dists={} (rep0-3: {n_rep}) | lit_bits={lit_bits} cmd_bits={} dist_bits={}",
            stream.cmd_symbols.len(),
            stream.literals.len(),
            stream.dist_symbols.len(),
            cmd_sym_bits + cmd_extra_bits,
            dist_sym_bits + dist_extra_bits
        );
        {
            // True bit split under the emitted block/context trees.
            let mut cmd_split_bits = 0u64;
            for (bi, freq) in cmd_freqs_per_block.iter().enumerate() {
                let lens = &cmd_lengths_per_block[bi].lengths;
                for (s, &f) in freq.iter().enumerate() {
                    if f > 0 {
                        cmd_split_bits += u64::from(f) * u64::from(lens[s]);
                    }
                }
            }
            let mut dist_split_bits = 0u64;
            for (ti, freq) in dist_freqs_per_ctx.iter().enumerate() {
                let lens = &dist_lengths_per_ctx[ti].lengths;
                for (s, &f) in freq.iter().enumerate() {
                    if f > 0 {
                        dist_split_bits += u64::from(f) * u64::from(lens[s]);
                    }
                }
            }
            eprintln!(
                "STATS split: cmd_sym={} cmd_extra={} dist_sym={} dist_extra={} lit={} blocks={} dtrees={}",
                cmd_split_bits,
                cmd_extra_bits,
                dist_split_bits,
                dist_extra_bits,
                lit_bits,
                cmd_boundaries.len(),
                ntrees_d
            );
        }
        eprintln!("STATS top distances: {:?}", &top);
    }

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
    // Flat single-table view for the emission fast paths: one bounds
    // check per literal instead of Vec-of-Vec double indirection.
    let lit_codes_flat: Vec<(u32, u8)> = {
        let mut f = Vec::with_capacity(ntrees * 256);
        for t in &lit_codes_per_tree {
            f.extend_from_slice(t);
        }
        f
    };
    let cmd_codes_per_block: Vec<Vec<(u32, u8)>> = cmd_lengths_per_block
        .iter()
        .map(canonical_with_reverse)
        .collect();
    let cmd_codes = canonical_with_reverse(&cmd_lengths);
    let dist_codes_per_ctx: Vec<Vec<(u32, u8)>> = dist_lengths_per_ctx
        .iter()
        .map(canonical_with_reverse)
        .collect();
    let dist_codes = canonical_with_reverse(&dist_lengths);

    // Write literal tree group (one table per tree).
    if env_flag!("BROTLI_DUMP_LITTREE") {
        for (ti, tree) in lit_lengths_per_tree.iter().enumerate() {
            let lens: Vec<String> = tree
                .lengths
                .iter()
                .enumerate()
                .filter(|(_, &l)| l > 0)
                .map(|(s2, &l)| format!("{s2}:{l}"))
                .collect();
            eprintln!("LITTREE {ti} {}", lens.join(","));
        }
        eprintln!("LITCMAP ntrees={ntrees_l} map={:?}", lit_ctx_map);
    }
    for tree in &lit_lengths_per_tree {
        write_huffman_table(bw, tree, 256);
    }
    if env_flag!("BROTLI_DUMP_CMDTREE") {
        for tree in &cmd_lengths_per_block {
            let lens: Vec<String> = tree
                .lengths
                .iter()
                .enumerate()
                .filter(|(_, &l)| l > 0)
                .map(|(s, &l)| format!("{s}:{l}"))
                .collect();
            eprintln!("CMDTREE {}", lens.join(","));
        }
    }
    for tree in &cmd_lengths_per_block {
        write_huffman_table(bw, tree, 704);
    }
    for (ti, tree) in dist_lengths_per_ctx.iter().enumerate() {
        if env_flag!("BROTLI_DBG_DC") {
            let lens: Vec<String> = tree
                .lengths
                .iter()
                .enumerate()
                .filter(|(_, &l)| l > 0)
                .map(|(s, &l)| format!("{s}:{l}"))
                .collect();
            eprintln!("DCTREE[{ti}] lens={}", lens.join(","));
        }
        write_huffman_table(bw, tree, dist_alphabet);
    }

    // --- Encode commands + literals with per-context tree selection ---
    let mut dist_iter = stream.dist_symbols.iter().zip(stream.dist_extras.iter());
    (p1, p2) = ctx_in;
    let mut lit_idx = 0usize;
    out_pos = 0;
    let mut lit_blk = 0usize;
    let mut lit_block_remaining: usize =
        lit_block_len.first().copied().unwrap_or(u32::MAX) as usize;
    let mut enc_cmd_n = 0usize;
    let mut lit_next_switch = 1usize;
    let mut cmd_block_remaining: usize =
        cmd_block_len.first().copied().unwrap_or(u32::MAX) as usize;
    let mut next_switch = 1usize; // index into cmd_boundaries/block types
    let mut dist_blk = 0usize;
    let mut dist_sym_idx = 0usize;
    let mut dist_block_remaining: usize =
        dist_block_len.first().copied().unwrap_or(u32::MAX) as usize;
    let mut dist_next_switch = 1usize;
    for (cmd_idx, (&cmd_sym, cmd)) in stream.cmd_symbols.iter().zip(commands.iter()).enumerate() {
        if cmd_idx > 0 && cmd_block_remaining == 0 && next_switch < cmd_boundaries.len() {
            // Block switch: explicit type code (type + 2), then block length.
            let new_type = *cmd_block_types
                .get(next_switch)
                .unwrap_or(&(next_switch as u8)) as usize;
            let (bt_code, bt_len) = cmd_bt_wire[new_type + 2];
            bw.write_bits(bt_code, u32::from(bt_len));
            let (c, extra, nbits) = block_length_code(cmd_block_len[next_switch]);
            let (bl_code, bl_len) = cmd_bl_wire[c];
            bw.write_bits(bl_code, u32::from(bl_len));
            bw.write_bits(extra, nbits);
            if env_flag!("BROTLI_SWITCH_LOG") {
                eprintln!(
                    "ENCSW-CMD n={cmd_idx} pos={mlen_offset}+{out_pos} type={new_type} len={}",
                    cmd_block_len[next_switch]
                );
            }
            cmd_block_remaining = cmd_block_len[next_switch] as usize;
            next_switch += 1;
        }
        let block = if nbltypes_c > 1 {
            let cd_block = usize::from(
                *cmd_block_types
                    .get(next_switch.saturating_sub(1))
                    .unwrap_or(&(next_switch.saturating_sub(1) as u8)),
            );
            let arr_block = usize::from(cmd_block_of[cmd_idx]);
            if arr_block != cd_block && std::env::var("BROTLI_CMDBLK_DBG").is_ok() {
                eprintln!("CMDBLK-DIVERGE cmd={cmd_idx} arr={arr_block} countdown={cd_block}");
            }
            arr_block
        } else {
            0
        };
        let cmd_table = if nbltypes_c > 1 {
            &cmd_codes_per_block[block]
        } else {
            &cmd_codes
        };
        let (code, len) = cmd_table[cmd_sym];
        if env_flag!("BROTLI_SYM_TRACE") && cmd_idx >= 230 && cmd_idx <= 240 {
            let hist_val = cmd_freqs_per_block.get(block).map_or(0, |h| h[cmd_sym]);
            eprintln!(
                "ENCSYM-CODE {cmd_idx} sym={cmd_sym} code={code} len={len} block={block} freq={hist_val}"
            );
        }
        bw.write_bits(code, u32::from(len));
        if nbltypes_c > 1 {
            cmd_block_remaining = cmd_block_remaining.saturating_sub(1);
        }

        let entry = &kCmdLut[cmd_sym];
        if entry.insert_len_extra_bits > 0 {
            let extra = cmd.insert_len - u32::from(entry.insert_len_offset);
            bw.write_bits(extra, u32::from(entry.insert_len_extra_bits));
        }
        if entry.copy_len_extra_bits > 0 {
            let extra = cmd.copy_len - u32::from(entry.copy_len_offset);
            bw.write_bits(extra, u32::from(entry.copy_len_extra_bits));
        }

        // Fast path: no literal block switches — the common shape for
        // binary inputs (4-tree LSB6 map) and every switch-free
        // metablock. Skips the block-switch countdown and the
        // Vec-of-Vec code lookup; the single-tree case skips the
        // context computation entirely.
        if nbltypes_l <= 1 {
            let literal_slice = &stream.literals[..];
            if ntrees == 1 {
                for _ in 0..cmd.insert_len {
                    let b = literal_slice[lit_idx];
                    let (lc, ll) = lit_codes_flat[usize::from(b)];
                    bw.write_bits(lc, u32::from(ll));
                    p2 = p1;
                    p1 = b;
                    lit_idx += 1;
                    out_pos += 1;
                }
            } else {
                for _ in 0..cmd.insert_len {
                    let b = literal_slice[lit_idx];
                    let ctx = compute_context_id(p1, p2, context_mode) as usize;
                    let tree = usize::from(lit_ctx_map[ctx]);
                    let (lc, ll) = lit_codes_flat[(tree << 8) + usize::from(b)];
                    bw.write_bits(lc, u32::from(ll));
                    p2 = p1;
                    p1 = b;
                    lit_idx += 1;
                    out_pos += 1;
                }
            }
        } else {
            for _ in 0..cmd.insert_len {
                // Literal block switch BEFORE the literal (decoder checks
                // block_length == 0 at the start of each literal read).
                if nbltypes_l > 1
                    && lit_block_remaining == 0
                    && lit_next_switch < lit_boundaries.len()
                {
                    let new_type = *lit_block_types
                        .get(lit_next_switch)
                        .unwrap_or(&(lit_next_switch as u8))
                        as usize;
                    if env_flag!("BROTLI_SW_TRACE") {
                        eprintln!(
                        "ENCSW n={lit_next_switch} type={new_type} len={} litpos={lit_idx} bit={}",
                        lit_block_len[lit_next_switch],
                        bw.out.len() * 8 + bw.nbits as usize
                    );
                    }
                    let (bt_code, bt_len) = lit_bt_wire[new_type + 2];
                    bw.write_bits(bt_code, u32::from(bt_len));
                    let (c, extra, nbits) = block_length_code(lit_block_len[lit_next_switch]);
                    let (bl_code, bl_len) = lit_bl_wire[c];
                    bw.write_bits(bl_code, u32::from(bl_len));
                    bw.write_bits(extra, nbits);
                    lit_blk = lit_next_switch;
                    lit_block_remaining = lit_block_len[lit_next_switch] as usize;
                    lit_next_switch += 1;
                }

                let b = stream.literals[lit_idx];
                if env_flag!("BROTLI_WALK_TRACE") {
                    let w = walk_assign.get(lit_idx);
                    let ctx = compute_context_id(p1, p2, context_mode) as usize;
                    let blk_ty = *lit_block_types.get(lit_blk).unwrap_or(&(lit_blk as u8)) as usize;
                    match w {
                        Some(&(wrow, wbyte)) if wrow != (blk_ty << 6) + ctx || wbyte != b => {
                            eprintln!(
                            "WALK-DIVERGE lit={lit_idx} walk_row={wrow} walk_byte={wbyte} emit_row={} emit_byte={b}",
                            (blk_ty << 6) + ctx
                        );
                        }
                        None => eprintln!("WALK-MISS lit={lit_idx}"),
                        _ => {}
                    }
                }
                let tree = if nbltypes_l > 1 {
                    let ctx = compute_context_id(p1, p2, context_mode) as usize;
                    let blk_ty = *lit_block_types.get(lit_blk).unwrap_or(&(lit_blk as u8)) as usize;
                    lit_ctx_map[(blk_ty << 6) + ctx] as usize
                } else if ntrees > 1 {
                    let ctx = compute_context_id(p1, p2, context_mode) as usize;
                    lit_ctx_map[ctx] as usize
                } else {
                    0
                };
                let (lc, ll) = lit_codes_per_tree[tree][b as usize];
                if env_flag!("BROTLI_DBG_CTX") && u32::from(ll) == 0 {
                    LIT0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                let _trace_lit = env_flag!("BROTLI_LIT_TRACE");
                if _trace_lit {
                    eprintln!(
                    "ENCLIT {lit_idx} bit={} tree={tree} len={ll} byte={b} p1={p1} p2={p2} blk={lit_blk}",
                    bw.out.len() * 8 + bw.nbits as usize
                );
                }
                bw.write_bits(lc, u32::from(ll));
                p2 = p1;
                p1 = b;
                lit_idx += 1;
                out_pos += 1;

                if nbltypes_l > 1 {
                    lit_block_remaining = lit_block_remaining.saturating_sub(1);
                }
            }
        }

        if env_flag!("BROTLI_CMD_TRACE") {
            eprintln!(
                "ENCCMD {enc_cmd_n} ins={} cpy={} dist={} outpos={out_pos} lit={lit_idx}",
                cmd.insert_len, cmd.copy_len, cmd.distance
            );
        }
        enc_cmd_n += 1;
        if cmd.copy_len > 0 {
            // Check if this command uses implicit distance (rep code).
            // Implicit commands don't have a distance symbol in the stream.
            let cmd_entry = &kCmdLut[cmd_sym];
            if cmd_entry.distance_code < 0 {
                let (&d_sym, &d_extra) = dist_iter.next().expect("distance stream exhausted");
                if nbltypes_d > 1 {
                    dist_sym_idx += 1;
                    if dist_block_remaining == 0 && dist_next_switch < dist_boundaries.len() {
                        let new_type = *dist_block_types
                            .get(dist_next_switch)
                            .unwrap_or(&(dist_next_switch as u8))
                            as usize;
                        let (bt_code, bt_len) = dist_bt_wire[new_type + 2];
                        bw.write_bits(bt_code, u32::from(bt_len));
                        let (c, extra, nbits) = block_length_code(dist_block_len[dist_next_switch]);
                        let (bl_code, bl_len) = dist_bl_wire[c];
                        bw.write_bits(bl_code, u32::from(bl_len));
                        bw.write_bits(extra, nbits);
                        if env_flag!("BROTLI_SWITCH_LOG") {
                            eprintln!(
                                "SW-DIST pos={mlen_offset}+{out_pos} type={new_type} len={}",
                                dist_block_len[dist_next_switch]
                            );
                        }
                        dist_blk = dist_next_switch;
                        dist_block_remaining = dist_block_len[dist_next_switch] as usize;
                        dist_next_switch += 1;
                    }
                    dist_block_remaining = dist_block_remaining.saturating_sub(1);
                }
                let table = if ntrees_d > 1 {
                    let ctx = if cmd.copy_len > 4 {
                        3u8
                    } else {
                        (cmd.copy_len - 2) as u8
                    };
                    &dist_codes_per_ctx[dist_ctx_tree_of(dist_blk, ctx)]
                } else {
                    &dist_codes
                };
                let (dc, dl) = table[d_sym as usize];
                if env_flag!("BROTLI_DIST_TRACE") && cmd_idx >= 230 && cmd_idx <= 240 {
                    let ctx = if cmd.copy_len > 4 {
                        3u8
                    } else {
                        (cmd.copy_len - 2) as u8
                    };
                    eprintln!(
                        "ENCDIST {cmd_idx} val={} sym={d_sym} extra={d_extra} ctx={ctx} code={dc} len={dl} bit={}",
                        cmd.distance,
                        bw.out.len() * 8 + bw.nbits as usize
                    );
                }
                if env_flag!("BROTLI_DBG_DC") {
                    eprintln!(
                        "DCWRITE sym={d_sym} code={dc:0b} len={dl} tree_idx={}",
                        if ntrees_d > 1 {
                            let ctx = if cmd.copy_len > 4 {
                                3u8
                            } else {
                                (cmd.copy_len - 2) as u8
                            };
                            match ntrees_d {
                                2 => usize::from(ctx >= 2),
                                _ => ctx as usize,
                            }
                        } else {
                            0
                        }
                    );
                }
                bw.write_bits(dc, u32::from(dl));
                let nbits = distance_extra_bits(d_sym, &dist_cfg);
                if nbits > 0 {
                    bw.write_bits(d_extra, nbits);
                }
            }
            if env_flag!("BROTLI_ADV_DBG") && cmd_copy_advances[cmd_idx] != cmd.copy_len as usize {
                eprintln!(
                    "ADV-DIFF cmd={cmd_idx} copy_len={} advance={}",
                    cmd.copy_len, cmd_copy_advances[cmd_idx]
                );
            }
            out_pos += cmd_copy_advances[cmd_idx];
            if out_pos > 0 && out_pos <= input.len() {
                // Mirror the decoder exactly (see frequency-collection
                // loop): p2 = second-to-last copied byte for copies ≥ 2.
                let new_p1 = input[out_pos - 1];
                p2 = if cmd.copy_len > 1 {
                    input[out_pos - 2]
                } else {
                    p1
                };
                p1 = new_p1;
            }
        }
    }
    if env_flag!("BROTLI_DBG_CTX") {
        eprintln!(
            "LIT0-final: zero-bit literals: {}",
            LIT0.load(std::sync::atomic::Ordering::Relaxed)
        );
    }
}

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
pub(crate) fn write_huffman_table(
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
        3 => {
            lengths.lengths[nonzero[0]] == 1
                && lengths.lengths[nonzero[1]] == 2
                && lengths.lengths[nonzero[2]] == 2
        }
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
    for (i, &sym) in CODE_LENGTH_CODE_ORDER.iter().enumerate() {
        let len = cl_lengths.lengths[usize::from(sym)];
        let (wire, nbits) = CL_CODE_TO_WIRE[usize::from(len)];
        if env_flag!("BROTLI_TREEDBG") {
            eprintln!("WRHD i={i} v={len} bits={wire}/{nbits}");
        }
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
    let tdbg = env_flag!("BROTLI_TREEDBG");
    for &(sym, extra) in &rle {
        if tdbg {
            eprintln!("WRCL sym={sym} extra={extra} space={main_space}");
        }
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
