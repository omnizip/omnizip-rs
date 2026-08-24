//! ZSTD compressed-block encoder. Orchestrates the match finder,
//! literals encoder, and sequences encoder into a `Compressed_Block`
//! (`block_type` = 2).
//!
//! For each 128 KiB chunk of input:
//! 1. Run `match_finder::compress_block_fast` → `SeqStore`.
//! 2. Encode literals section (Raw, RLE, or Huffman).
//! 3. Encode sequences section (Predefined FSE tables).
//! 4. Choose Raw / RLE / Compressed, whichever produces the smallest
//!    block content.

#![forbid(unsafe_code)]

use crate::constants::{BLOCK_TYPE_COMPRESSED, BLOCK_TYPE_RAW, BLOCK_TYPE_RLE};
use crate::encoder::ldm::LdmHashTable;
use crate::encoder::match_finder::{
    compress_block_fast_with_prefix, compress_block_lazy, compress_block_lazy2,
    compress_block_lazy2_with_ldm, compress_block_lazy2_with_prefix,
    compress_block_lazy_with_prefix, compress_block_with_min_match, MatchState, SeqStore,
};
use crate::encoder::sequences::encode_section;
use crate::xxhash;
use crate::ZstdError;

/// Maximum block content size (128 KiB per ZSTD spec). Use 127 KiB to
/// avoid edge cases where some decoders reject exactly-128KiB blocks.
pub(crate) const BLOCK_MAX_SIZE: usize = 127 * 1024;

/// Total-variation distance between the byte histograms of two
/// slices, above 0.25 meaning "different content regimes".
fn halves_diverge(a: &[u8], b: &[u8]) -> bool {
    let mut ha = [0u32; 256];
    let mut hb = [0u32; 256];
    for &x in a {
        ha[x as usize] += 1;
    }
    for &x in b {
        hb[x as usize] += 1;
    }
    let (la, lb) = (a.len() as f64, b.len() as f64);
    let mut tvd = 0.0f64;
    for i in 0..256 {
        tvd += (f64::from(ha[i]) / la - f64::from(hb[i]) / lb).abs();
    }
    tvd > 0.5
}

/// Sparse sampling gap for the LDM hash table (1 entry per 64 bytes).
/// Controls the memory/coverage trade-off: smaller = denser sampling
/// (more memory, finds more matches); larger = sparser (less memory,
/// may miss some matches).
const LDM_GAP: usize = 64;

/// Determine whether LDM should be enabled for this input.
///
/// LDM is enabled at Btultra2 strategy (L19+) when the input exceeds
/// one block size, so cross-block long-distance matches are possible.
/// Below L19 or for small inputs, LDM adds overhead with no benefit.
fn should_enable_ldm(
    params: &crate::encoder::cparams::CompressionParams,
    input_len: usize,
) -> bool {
    use crate::encoder::cparams::Strategy;
    input_len > BLOCK_MAX_SIZE && matches!(params.strategy, Strategy::Btultra2)
}

/// Minimum hash log (matches ZSTD's `HASH_LOG_MIN`).
const HASH_LOG_MIN: u32 = 6;
/// Maximum hash log (matches ZSTD's `HASH_LOG_MAX_32`).
const HASH_LOG_MAX: u32 = 25;

/// Cap the requested `hash_log` based on input size, mirroring
/// `ZSTD_adjustCParams_internal` in the C reference. The "default"
/// table assumes input ≥ 256 KB; for smaller inputs we shrink the
/// hash table so allocation cost (and zero-init cost) doesn't dominate.
///
/// Rule of thumb: hash table size never exceeds the input size. For a
/// 4 KB input, this caps `hash_log` at 12 (4 KB table) instead of 25
/// (128 MB table).
/// Cap the requested `hash_log` based on input size, mirroring
/// `ZSTD_adjustCParams_internal` in the C reference. The "default"
/// table assumes input ≥ 256 KB; for smaller inputs we shrink the
/// hash table so allocation cost (and zero-init cost) doesn't dominate.
///
/// Rule of thumb: hash table size never exceeds the input size. For a
/// 4 KB input, this caps `hash_log` at 12 (4 KB table) instead of 25
/// (128 MB table).
#[must_use]
pub fn cap_hash_log_for_input(hash_log: u32, input_len: usize) -> u32 {
    if input_len == 0 {
        return HASH_LOG_MIN;
    }
    // floor(log2(input_len)) — the largest hash table worth allocating
    // for this input size.
    let input_log = u32::try_from(64 - (input_len as u64).leading_zeros().saturating_sub(1))
        .unwrap_or(HASH_LOG_MIN);
    hash_log.min(input_log).max(HASH_LOG_MIN).min(HASH_LOG_MAX)
}

/// Encode `plaintext` as a complete ZSTD frame with compressed blocks.
///
/// Compressed block encoder: match finder + FSE sequences + Huffman/Raw literals.
/// blocks. The output is a valid ZSTD frame that round-trips through
/// any decoder.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on internal failures.
pub fn encode_frame_compressed(plaintext: &[u8], level: u8) -> Result<Vec<u8>, ZstdError> {
    let mut params = crate::encoder::cparams::get_params(level);
    params.hash_log = cap_hash_log_for_input(params.hash_log, plaintext.len());
    let mut out = Vec::with_capacity(plaintext.len() / 2 + 64);
    let mut match_state = MatchState::new(params.hash_log);
    encode_frame_into(&mut out, plaintext, &params, &mut match_state)?;
    Ok(out)
}

/// Like [`encode_frame_compressed`] but writes into a caller-provided
/// `out` vector and uses a caller-provided `MatchState`. The match
/// state is cleared on entry (so the caller can reuse it across calls
/// via [`crate::ZstdCompressor`]).
///
/// `params.hash_log` should already be capped to the input size; the
/// caller is responsible for that (see [`cap_hash_log_for_input`]).
pub fn encode_frame_into_pub(
    out: &mut Vec<u8>,
    plaintext: &[u8],
    params: &crate::encoder::cparams::CompressionParams,
    match_state: &mut MatchState,
) -> Result<(), ZstdError> {
    encode_frame_into(out, plaintext, params, match_state)
}

/// Like [`encode_frame_compressed`] but writes into a caller-provided
/// `out` vector and uses a caller-provided `MatchState`. The match
/// state is cleared on entry (so the caller can reuse it across calls
/// via [`crate::ZstdCompressor`]).
///
/// `params.hash_log` should already be capped to the input size; the
/// caller is responsible for that (see [`cap_hash_log_for_input`]).
fn encode_frame_into(
    out: &mut Vec<u8>,
    plaintext: &[u8],
    params: &crate::encoder::cparams::CompressionParams,
    match_state: &mut MatchState,
) -> Result<(), ZstdError> {
    match_state.clear();

    // Magic.
    out.extend_from_slice(&crate::constants::MAGIC_BYTES);

    // Frame header: descriptor + optional window_descriptor + FCS.
    write_frame_header(out, plaintext.len(), None);

    // LDM is enabled at Btultra2 (L19+) for inputs larger than one
    // block. The LDM hash table is pre-populated over the full input
    // and queried at every position alongside the normal hash table.
    let ldm_enabled = should_enable_ldm(params, plaintext.len());

    // Build and pre-populate the LDM hash table.
    let ldm_table: Option<LdmHashTable> = if ldm_enabled {
        let mut ldm = LdmHashTable::new(params.window_log, LDM_GAP);
        for pos in 0..plaintext.len() {
            ldm.insert(plaintext, pos);
        }
        Some(ldm)
    } else {
        None
    };

    // In LDM mode, the hash table is NOT cleared between blocks
    // (positions are absolute across the full frame). Chain walking
    // is disabled because the chain table is sized for one block;
    // LDM provides long-distance coverage instead.
    if ldm_enabled {
        match_state.disable_chain();
    }

    // Cross-block matching for ALL non-LDM strategies: use absolute
    // positions with `_with_prefix` match finders. The hash table
    // persists across blocks, enabling matches up to 128 KiB back
    // (including previous blocks).
    let cross_block = !ldm_enabled;

    if cross_block {
        match_state.disable_chain();
    }

    // In LDM mode, the distance cap is the full frame window. For
    // single-segment frames (input ≤ 4 GiB), the window = FCS =
    // input length, so any backward reference is valid.
    let max_distance = if ldm_enabled { plaintext.len() } else { 0 };

    // Blocks.
    let mut rep_offsets = [1u32, 4, 8];
    let mut last_huf_weights: Option<Vec<u8>> = None;
    let mut offset = 0;
    while offset < plaintext.len() {
        let remaining = plaintext.len() - offset;
        let chunk_size = remaining.min(BLOCK_MAX_SIZE);
        let is_last = offset + chunk_size == plaintext.len();
        let block_end = offset + chunk_size;

        // Adaptive block splitting (a coarse stand-in for the
        // reference's entropy-based splitter): heterogeneous chunks —
        // first and second halves' byte distributions diverge — get
        // 16 KiB sub-blocks so each Huffman/FSE table fits its region.
        // Homogeneous chunks (repetitive, uniform-random, or stationary
        // text) keep 128 KiB blocks and their minimal header overhead.
        // Threshold 0.25 total-variation distance separates cleanly:
        // FITS headers-vs-data ~0.5; repetitive/random/stationary
        // fixtures <= 0.024.
        let sub_split = chunk_size >= 32 * 1024 && {
            let mid = chunk_size / 2;
            halves_diverge(
                &plaintext[offset..offset + mid],
                &plaintext[offset + mid..block_end],
            )
        };
        let step = if sub_split { 16 * 1024 } else { chunk_size };

        if ldm_enabled {
            let mut sub = offset;
            while sub < block_end {
                let sub_end = (sub + step).min(block_end);
                let sub_last = is_last && sub_end == block_end;
                write_block_ldm(
                    out,
                    plaintext,
                    sub,
                    sub_end,
                    sub_last,
                    match_state,
                    &mut rep_offsets,
                    params,
                    &mut last_huf_weights,
                    ldm_table
                        .as_ref()
                        .expect("ldm table exists when ldm_enabled"),
                    max_distance,
                )?;
                sub = sub_end;
            }
        } else if cross_block {
            let mut sub = offset;
            while sub < block_end {
                let sub_end = (sub + step).min(block_end);
                let sub_last = is_last && sub_end == block_end;
                write_block_cross(
                    out,
                    plaintext,
                    sub,
                    sub_end,
                    sub_last,
                    match_state,
                    &mut rep_offsets,
                    params,
                    &mut last_huf_weights,
                )?;
                sub = sub_end;
            }
        } else {
            let mut sub = offset;
            while sub < block_end {
                let sub_end = (sub + step).min(block_end);
                let sub_last = is_last && sub_end == block_end;
                write_block(
                    out,
                    &plaintext[sub..sub_end],
                    sub_last,
                    match_state,
                    &mut rep_offsets,
                    params,
                    &mut last_huf_weights,
                )?;
                sub = sub_end;
            }
        }
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

    Ok(())
}

/// Encode `plaintext` as a complete ZSTD frame primed with a
/// dictionary prefix.
///
/// Phase 1 strategy: build a virtual stream `dict_content ++
/// plaintext`, seed the match finder's hash table with the dictionary
/// positions, then compress the plaintext region only. Matches may
/// back-reference dictionary bytes. The resulting frame:
///
/// - Has `Frame_Content_Size` = `plaintext.len()` (NOT including the
///   dictionary).
/// - Has a content checksum over `plaintext` only.
/// - Carries the dictionary's `id` in the frame header.
///
/// The frame is **not** decodable by a standalone ZSTD decoder — it
/// requires the dict-aware path ([`crate::decompress_with_dict`])
/// which primes the decoder's output window with the dictionary
/// content before executing sequences.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on internal failures.
pub fn encode_frame_with_dict(
    plaintext: &[u8],
    level: u8,
    dict: &crate::dict::ZstdDictionary,
) -> Result<Vec<u8>, ZstdError> {
    use crate::encoder::cparams::Strategy;
    use crate::encoder::match_finder::{
        compress_block_fast_with_prefix, compress_block_lazy2_with_prefix,
        compress_block_lazy_with_prefix,
    };

    let dict_content = dict.content();
    let mut params = crate::encoder::cparams::get_params(level);
    params.hash_log = cap_hash_log_for_input(params.hash_log, dict_content.len() + plaintext.len());

    // Virtual stream: dict_content ++ plaintext. Match positions are
    // absolute within this buffer.
    let mut virtual_stream: Vec<u8> = Vec::with_capacity(dict_content.len() + plaintext.len());
    virtual_stream.extend_from_slice(dict_content);
    let prefix_len = virtual_stream.len();
    virtual_stream.extend_from_slice(plaintext);

    let mut out = Vec::with_capacity(plaintext.len() / 2 + 64);
    let mut match_state = MatchState::new(params.hash_log);

    // Seed the hash table with dictionary positions.
    if prefix_len >= 4 {
        match_state.seed_prefix(&virtual_stream, prefix_len);
    }

    // Magic.
    out.extend_from_slice(&crate::constants::MAGIC_BYTES);

    // Frame header: FCS = plaintext.len(), with Dictionary_ID.
    write_frame_header(&mut out, plaintext.len(), Some(dict.id()));

    // Blocks: iterate plaintext in BLOCK_MAX_SIZE chunks, but pass
    // the full virtual_stream slice + prefix_len to the dict-aware
    // match finders. The hash table persists across blocks (no
    // ms.clear()) so dictionary positions remain queryable.
    let mut rep_offsets = [1u32, 4, 8];
    let mut last_huf_weights: Option<Vec<u8>> = None;
    let mut offset = prefix_len;
    let end = virtual_stream.len();

    if offset < end {
        loop {
            let remaining = end - offset;
            let chunk_size = remaining.min(BLOCK_MAX_SIZE);
            let is_last = offset + chunk_size == end;

            // Run the dict-aware match finder over the virtual
            // stream starting at `offset`, producing a SeqStore for
            // just this block's plaintext region.
            let mut seq_store = SeqStore::new();
            seq_store.reset(rep_offsets);
            let min_match = params.min_match.max(5) as usize;

            match params.strategy {
                Strategy::Fast | Strategy::DoubleFast | Strategy::Greedy => {
                    // For Fast/DoubleFast/Greedy, the fast parser
                    // suffices (no look-ahead).
                    compress_block_fast_with_prefix(
                        &virtual_stream[..offset + chunk_size],
                        offset,
                        &mut seq_store,
                        &mut match_state,
                        min_match,
                    );
                }
                Strategy::Lazy => {
                    compress_block_lazy_with_prefix(
                        &virtual_stream[..offset + chunk_size],
                        offset,
                        &mut seq_store,
                        &mut match_state,
                        min_match,
                    );
                }
                _ => {
                    compress_block_lazy2_with_prefix(
                        &virtual_stream[..offset + chunk_size],
                        offset,
                        &mut seq_store,
                        &mut match_state,
                        min_match,
                    );
                }
            }
            let block_initial_reps = rep_offsets;

            // The chunk for literal/RLE/block-header purposes is the
            // plaintext slice [offset, offset+chunk_size).
            let chunk = &virtual_stream[offset..offset + chunk_size];

            // RLE check.
            if chunk.len() >= 2 && chunk.iter().all(|&b| b == chunk[0]) {
                write_rle_block(&mut out, chunk[0], chunk.len(), is_last);
                last_huf_weights = None;
            } else {
                let mut compressed_content = Vec::new();
                let encode_result = encode_compressed_content(
                    &mut compressed_content,
                    &seq_store,
                    &mut last_huf_weights,
                    block_initial_reps,
                );
                let encode_ok = encode_result.is_ok();

                if encode_ok && compressed_content.len() < chunk.len() {
                    // Wire rep state for the next block: the sequence
                    // encoder's final state (raw blocks leave the
                    // decoder's rep slots at `block_initial_reps`).
                    rep_offsets = encode_result.as_ref().map_or(block_initial_reps, |&r| r);
                    write_compressed_block_header(&mut out, compressed_content.len(), is_last);
                    out.extend_from_slice(&compressed_content);
                } else {
                    write_raw_block(&mut out, chunk, is_last);
                    last_huf_weights = None;
                }
            }

            offset += chunk_size;
            if offset >= end {
                break;
            }
        }
    } else {
        // Plaintext is empty: emit a single empty last Raw block.
        let hdr: u32 = 1;
        out.extend_from_slice(&hdr.to_le_bytes()[..3]);
    }

    // Content checksum over plaintext only.
    let checksum = xxhash::zstd_frame_checksum(plaintext);
    out.extend_from_slice(&checksum.to_le_bytes());

    Ok(out)
}

/// Write the frame header using the smallest FCS encoding that fits,
/// matching the C reference's priority order.
///
/// Descriptor byte layout:
/// - bits 0-1: `Dictionary_ID_flag` (0 = no `Dict_ID`, 1/2/3 = 1/2/4-byte ID).
/// - bit 2: `Content_Checksum_flag` (always 1 — we always emit the checksum).
/// - bit 5: `Single_Segment_flag`.
/// - bits 6-7: `Frame_Content_Size_flag` (0/1/2/3 → 0/2/4/8-byte FCS).
///
/// Encoding choice:
/// - size ≤ 255: `FCS_flag=0` (1 byte), `single_segment=1` → 2-byte header.
/// - size ≤ 65535: `FCS_flag=1` (2 bytes), `single_segment=1` → 3-byte header.
/// - size ≤ 2³²-1: `FCS_flag=2` (4 bytes), `single_segment=1` → 5-byte header.
/// - larger: `FCS_flag=3` (8 bytes), `window_descriptor` → 10-byte header.
///
/// When `single_segment=1`, no `window_descriptor` is emitted (the window
/// size is implied to equal the frame content size).
///
/// When `dict_id` is `Some(id)`, the smallest `Dictionary_ID` encoding
/// that fits is emitted (1 byte for id ≤ 255, 2 bytes for id ≤ 65535,
/// 4 bytes otherwise). The ID value 0 is treated as "no dict id".
fn write_frame_header(out: &mut Vec<u8>, uncompressed_size: usize, dict_id: Option<u32>) {
    let size_u64 = uncompressed_size as u64;

    // Pick the smallest FCS encoding.
    let (fcs_flag, fcs_bytes_buf, single_segment): (u8, [u8; 8], bool) = if size_u64 <= 255 {
        (0, size_u64.to_le_bytes(), true)
    } else if size_u64 <= 65_791 {
        // FCS_Type=1: 2-byte field. Decoder adds 256, so subtract here.
        let stored = size_u64 - 256;
        (1, stored.to_le_bytes(), true)
    } else if u32::try_from(size_u64).is_ok() {
        (2, size_u64.to_le_bytes(), true)
    } else {
        // Need window_descriptor + 8-byte FCS for > 4 GiB inputs.
        let window_log: u32 = 64 - size_u64.saturating_sub(1).leading_zeros();
        let window_log = window_log.max(10).min(31);
        let window_descriptor: u8 = ((window_log - 10) as u8) << 3;
        // Fold the Dictionary_ID flag into the descriptor here too.
        let (did_flag, did_bytes) = dict_id_encoding(dict_id);
        let descriptor: u8 = (3u8 << 6) | 0x04 | did_flag;
        out.push(descriptor);
        out.push(window_descriptor);
        out.extend_from_slice(&did_bytes);
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

    let (did_flag, did_bytes) = dict_id_encoding(dict_id);
    let descriptor: u8 = (fcs_flag << 6)
        | 0x04 // content_checksum = 1
        | if single_segment { 0x20 } else { 0 }
        | did_flag;
    out.push(descriptor);
    out.extend_from_slice(&did_bytes);
    out.extend_from_slice(fcs_bytes);
}

/// Pick the smallest `Dictionary_ID` encoding for the given id.
/// Returns `(flag_bits, id_bytes)` where `flag_bits` goes into the
/// descriptor's bits 0-1 and `id_bytes` is the little-endian ID
/// payload (0/1/2/4 bytes).
fn dict_id_encoding(dict_id: Option<u32>) -> (u8, Vec<u8>) {
    match dict_id {
        None | Some(0) => (0, Vec::new()),
        Some(id) if id <= 0xFF => (1, vec![id as u8]),
        Some(id) if id <= 0xFFFF => (2, (id as u16).to_le_bytes().to_vec()),
        Some(id) => (3, id.to_le_bytes().to_vec()),
    }
}

/// Write one block. Chooses Raw/RLE/Compressed/Treeless based on which
/// produces the smallest output.
fn write_block(
    out: &mut Vec<u8>,
    chunk: &[u8],
    is_last: bool,
    ms: &mut MatchState,
    rep_offsets: &mut [u32; 3],
    params: &crate::encoder::cparams::CompressionParams,
    last_huf_weights: &mut Option<Vec<u8>>,
) -> Result<(), ZstdError> {
    let initial_reps = *rep_offsets;

    // Clear hash table: positions are block-relative, so cross-block
    // references would be invalid.
    ms.clear();

    // RLE check: entire chunk is one repeated byte.
    if chunk.len() >= 2 && chunk.iter().all(|&b| b == chunk[0]) {
        write_rle_block(out, chunk[0], chunk.len(), is_last);
        *last_huf_weights = None;
        return Ok(());
    }

    // Try compressed block.
    let mut seq_store = SeqStore::new();
    seq_store.reset(*rep_offsets);
    let min_match = params.min_match.max(5) as usize;

    // Configure match finder depth based on strategy.
    // search_log controls hash-chain walking depth (1 << search_log).
    use crate::encoder::cparams::Strategy;
    match params.strategy {
        Strategy::Fast | Strategy::DoubleFast => {
            ms.disable_chain();
        }
        Strategy::Greedy => {
            ms.disable_chain();
        }
        Strategy::Lazy => {
            ms.enable_chain(1 << params.search_log.min(4));
        }
        Strategy::Lazy2
        | Strategy::Btlazy2
        | Strategy::Btopt
        | Strategy::Btultra
        | Strategy::Btultra2 => {
            ms.enable_chain(1 << params.search_log);
        }
    }

    match params.strategy {
        Strategy::Fast | Strategy::DoubleFast => {
            compress_block_with_min_match(chunk, &mut seq_store, ms, min_match);
        }
        Strategy::Greedy => {
            compress_block_with_min_match(chunk, &mut seq_store, ms, min_match);
        }
        Strategy::Lazy => {
            compress_block_lazy(chunk, &mut seq_store, ms, min_match);
        }
        Strategy::Lazy2
        | Strategy::Btlazy2
        | Strategy::Btopt
        | Strategy::Btultra
        | Strategy::Btultra2 => {
            compress_block_lazy2(chunk, &mut seq_store, ms, min_match);
        }
    }

    let mut compressed_content = Vec::new();
    let encode_result = encode_compressed_content(
        &mut compressed_content,
        &seq_store,
        last_huf_weights,
        initial_reps,
    );
    let use_compressed = encode_result.is_ok() && compressed_content.len() < chunk.len();

    if use_compressed {
        // Wire rep state for the next block: the sequence encoder's
        // final state. A raw block discards the encoded sequences and
        // leaves the decoder's rep slots at `initial_reps`.
        *rep_offsets = encode_result.as_ref().map_or(initial_reps, |&r| r);
        write_compressed_block_header(out, compressed_content.len(), is_last);
        out.extend_from_slice(&compressed_content);
    } else {
        write_raw_block(out, chunk, is_last);
        *last_huf_weights = None;
    }

    Ok(())
}

/// Write one block using LDM. Like [`write_block`] but:
/// - Does NOT clear the hash table (positions are absolute across blocks).
/// - Uses [`compress_block_lazy2_with_ldm`] instead of the normal match finder.
/// - `src` is the full plaintext truncated to `[..block_end]`, so the
///   match finder can see all previous blocks' bytes for cross-block matches.
fn write_block_ldm(
    out: &mut Vec<u8>,
    plaintext: &[u8],
    block_start: usize,
    block_end: usize,
    is_last: bool,
    ms: &mut MatchState,
    rep_offsets: &mut [u32; 3],
    params: &crate::encoder::cparams::CompressionParams,
    last_huf_weights: &mut Option<Vec<u8>>,
    ldm: &LdmHashTable,
    max_distance: usize,
) -> Result<(), ZstdError> {
    let initial_reps = *rep_offsets;
    let chunk = &plaintext[block_start..block_end];

    // RLE check: entire chunk is one repeated byte.
    if chunk.len() >= 2 && chunk.iter().all(|&b| b == chunk[0]) {
        write_rle_block(out, chunk[0], chunk.len(), is_last);
        *last_huf_weights = None;
        return Ok(());
    }

    // Run the LDM-aware lazy2 match finder over this block.
    let mut seq_store = SeqStore::new();
    seq_store.reset(*rep_offsets);
    let min_match = params.min_match.max(5) as usize;

    let src = &plaintext[..block_end];
    compress_block_lazy2_with_ldm(
        src,
        block_start,
        &mut seq_store,
        ms,
        ldm,
        min_match,
        max_distance,
    );

    // Try compressed block, fall back to Raw.
    let mut compressed_content = Vec::new();
    let encode_result = encode_compressed_content(
        &mut compressed_content,
        &seq_store,
        last_huf_weights,
        initial_reps,
    );
    let use_compressed = encode_result.is_ok() && compressed_content.len() < chunk.len();

    if use_compressed {
        // Wire rep state for the next block: the sequence encoder's
        // final state. A raw block discards the encoded sequences and
        // leaves the decoder's rep slots at `initial_reps`.
        *rep_offsets = encode_result.as_ref().map_or(initial_reps, |&r| r);
        write_compressed_block_header(out, compressed_content.len(), is_last);
        out.extend_from_slice(&compressed_content);
    } else {
        write_raw_block(out, chunk, is_last);
        *last_huf_weights = None;
    }

    Ok(())
}

/// Write one block using cross-block matching (absolute positions).
/// Used for Fast/DoubleFast/Greedy strategies where single-probe
/// matching suffices and the hash table persists across blocks.
fn write_block_cross(
    out: &mut Vec<u8>,
    plaintext: &[u8],
    block_start: usize,
    block_end: usize,
    is_last: bool,
    ms: &mut MatchState,
    rep_offsets: &mut [u32; 3],
    params: &crate::encoder::cparams::CompressionParams,
    last_huf_weights: &mut Option<Vec<u8>>,
) -> Result<(), ZstdError> {
    let initial_reps = *rep_offsets;
    let chunk = &plaintext[block_start..block_end];

    if chunk.len() >= 2 && chunk.iter().all(|&b| b == chunk[0]) {
        write_rle_block(out, chunk[0], chunk.len(), is_last);
        *last_huf_weights = None;
        return Ok(());
    }

    let mut seq_store = SeqStore::new();
    seq_store.reset(*rep_offsets);
    let min_match = params.min_match.max(5) as usize;

    let src = &plaintext[..block_end];
    use crate::encoder::cparams::Strategy;
    match params.strategy {
        Strategy::Lazy => {
            compress_block_lazy_with_prefix(src, block_start, &mut seq_store, ms, min_match);
        }
        Strategy::Lazy2
        | Strategy::Btlazy2
        | Strategy::Btopt
        | Strategy::Btultra
        | Strategy::Btultra2 => {
            compress_block_lazy2_with_prefix(src, block_start, &mut seq_store, ms, min_match);
        }
        _ => {
            compress_block_fast_with_prefix(src, block_start, &mut seq_store, ms, min_match);
        }
    }

    let mut compressed_content = Vec::new();
    let encode_result = encode_compressed_content(
        &mut compressed_content,
        &seq_store,
        last_huf_weights,
        initial_reps,
    );
    let use_compressed = encode_result.is_ok() && compressed_content.len() < chunk.len();

    if use_compressed {
        // Wire rep state for the next block: the sequence encoder's
        // final state. A raw block discards the encoded sequences and
        // leaves the decoder's rep slots at `initial_reps`.
        *rep_offsets = encode_result.as_ref().map_or(initial_reps, |&r| r);
        write_compressed_block_header(out, compressed_content.len(), is_last);
        out.extend_from_slice(&compressed_content);
    } else {
        write_raw_block(out, chunk, is_last);
        *last_huf_weights = None;
    }

    Ok(())
}

/// Encode the compressed block content: literals section + sequences
/// section. Tries Raw, Huffman (Compressed), and Huffman (Treeless)
/// literals, picks the smallest.
fn encode_compressed_content(
    out: &mut Vec<u8>,
    seq_store: &SeqStore,
    last_huf_weights: &mut Option<Vec<u8>>,
    initial_reps: [u32; 3],
) -> Result<[u32; 3], ZstdError> {
    // Single-distinct-symbol literal sets: the RLE literals section
    // (block_type 01) — header + one byte. The Huffman path CANNOT
    // represent this: a Huffman table needs ≥ 2 symbols, and the old
    // degenerate fallback (weights over symbols {0,1}) produced codes
    // for the wrong symbols — the real literal byte encoded as a
    // zero-bit code and decoded as a flood of 0x00 (the 100 MB Best
    // corruption: LDM parses leave long single-byte literal runs).
    let single_symbol = seq_store
        .literals
        .first()
        .is_some_and(|&b| seq_store.literals.iter().all(|&x| x == b));

    // Build Raw literals section (always correct).
    let mut raw_literals = Vec::new();
    write_raw_literals(&mut raw_literals, &seq_store.literals);

    // Build Huffman literals section (Compressed, block_type=2).
    let (huf_literals, huf_weights) =
        match crate::huffman::encoder::encode_literals_with_weights(&seq_store.literals, false) {
            Ok((data, weights)) => (data, Some(weights)),
            Err(_) => (Vec::new(), None),
        };

    // Try Treeless (block_type=3) if previous block established a
    // Huffman table with identical weights.
    let treeless_literals = match (last_huf_weights.as_ref(), &huf_weights) {
        (Some(prev), Some(curr)) if prev == curr => {
            crate::huffman::encoder::encode_literals_with_weights(&seq_store.literals, true)
                .map(|(data, _)| data)
                .unwrap_or_default()
        }
        _ => Vec::new(),
    };

    // Pick the smallest literals representation.
    let raw_len = raw_literals.len();
    let huf_len = if huf_literals.is_empty() {
        None
    } else {
        Some(huf_literals.len())
    };
    let treeless_len = if treeless_literals.is_empty() {
        None
    } else {
        Some(treeless_literals.len())
    };

    // RLE literals (single distinct symbol) always win when present.
    if single_symbol {
        let b = seq_store.literals[0];
        write_rle_literals(out, b, seq_store.literals.len());
        // No Huffman table established for a successor Treeless block.
        *last_huf_weights = None;
    } else if let Some(tl) = treeless_len {
        if tl < raw_len && (huf_len.is_none() || tl <= huf_len.unwrap()) {
            out.extend_from_slice(&treeless_literals);
        } else if let Some(hl) = huf_len {
            if hl < raw_len {
                out.extend_from_slice(&huf_literals);
                *last_huf_weights = huf_weights;
            } else {
                out.extend_from_slice(&raw_literals);
                *last_huf_weights = None;
            }
        } else {
            out.extend_from_slice(&raw_literals);
            *last_huf_weights = None;
        }
    } else if let Some(hl) = huf_len {
        if hl < raw_len {
            out.extend_from_slice(&huf_literals);
            *last_huf_weights = huf_weights;
        } else {
            out.extend_from_slice(&raw_literals);
            *last_huf_weights = None;
        }
    } else {
        out.extend_from_slice(&raw_literals);
        *last_huf_weights = None;
    }

    // Sequences section.
    let mut final_reps = initial_reps;
    if seq_store.sequences.is_empty() && seq_store.literals.is_empty() {
        out.push(0x00);
    } else {
        final_reps = encode_section(out, seq_store, initial_reps)?;
    }

    Ok(final_reps)
}

/// Write a Raw literals section (`block_type=0`). Minimal header for
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

/// Write an RLE literals section (block_type 01): the size-format
/// header carries the REGENERATED size, followed by a single byte —
/// the literal repeated `lit_size` times. This is the format's
/// intended representation for a single-distinct-symbol literal set.
fn write_rle_literals(out: &mut Vec<u8>, byte: u8, lit_size: usize) {
    // Same size-format layout as Raw, with block_type bits = 01.
    if lit_size < 32 {
        out.push(((lit_size << 3) as u8) | 0x01);
    } else if lit_size < 4096 {
        let lhc: u16 = ((lit_size as u16) << 4) | 0x05; // lhl_code=1, type=01
        out.extend_from_slice(&lhc.to_le_bytes());
    } else {
        let lhc: u32 = ((lit_size as u32) << 4) | 0x0D; // lhl_code=3, type=01
        out.extend_from_slice(&lhc.to_le_bytes()[..3]);
    }
    out.push(byte);
}

/// Write a Raw block header (3 bytes LE) + data.
fn write_raw_block(out: &mut Vec<u8>, data: &[u8], is_last: bool) {
    let hdr: u32 =
        usize::from(is_last) as u32 | (u32::from(BLOCK_TYPE_RAW) << 1) | ((data.len() as u32) << 3);
    out.extend_from_slice(&hdr.to_le_bytes()[..3]);
    out.extend_from_slice(data);
}

/// Write an RLE block header + the repeated byte.
fn write_rle_block(out: &mut Vec<u8>, byte: u8, size: usize, is_last: bool) {
    let hdr: u32 =
        usize::from(is_last) as u32 | (u32::from(BLOCK_TYPE_RLE) << 1) | ((size as u32) << 3);
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
                if i % 100 < 50 {
                    (i % 26 + b'a' as i32) as u8
                } else {
                    (i % 256) as u8
                }
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
    fn strategy_dispatch_round_trips() {
        // Representative strategies: Fast (L1), Greedy (L5), Lazy (L6),
        // Lazy2 (L8). Each must produce decodable output.
        let input: Vec<u8> = (0..2_000)
            .map(|i| {
                if i % 50 < 40 {
                    b'a' + (i % 3) as u8
                } else {
                    (i % 256) as u8
                }
            })
            .collect();
        for &level in &[1u8, 5, 6, 8] {
            let compressed = encode_frame_compressed(&input, level)
                .unwrap_or_else(|e| panic!("encode L{level} failed: {e:?}"));
            let decompressed = decompress(&compressed, input.len() as u32)
                .unwrap_or_else(|e| panic!("decode L{level} failed: {e:?}"));
            assert_eq!(decompressed, input, "round-trip failed at L{level}");
        }
    }

    #[test]
    fn higher_levels_compress_better() {
        // Mixed content: some repetition, some unique bytes. The lazy
        // parser's look-ahead should find better match boundaries than
        // the greedy fast parser.
        let mut input = Vec::new();
        for block in 0..500 {
            // Each block has a shared prefix (repeated) + unique suffix.
            input.extend_from_slice(b"function process(");
            input.extend_from_slice(format!("{block}").as_bytes());
            input.extend_from_slice(b") {{ return data[");
            input.extend_from_slice(format!("{block}").as_bytes());
            input.extend_from_slice(b"]; }}\n");
        }
        let l1 = encode_frame_compressed(&input, 1).expect("L1");
        let l9 = encode_frame_compressed(&input, 9).expect("L9");
        assert!(
            l9.len() <= l1.len(),
            "L9 ({}) should be ≤ L1 ({})",
            l9.len(),
            l1.len()
        );
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

    #[test]
    fn ldm_round_trips_large_repetitive_input() {
        // 500 KiB input: larger than BLOCK_MAX_SIZE (127 KiB) so LDM
        // activates at L19+. The content has a repeating 4 KiB block
        // that LDM should find across block boundaries.
        let block: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        let input: Vec<u8> = block.repeat(128); // 512 KiB

        let compressed = encode_frame_compressed(&input, 19).expect("encode L19");
        let decompressed = decompress(&compressed, input.len() as u32).expect("decode L19");
        assert_eq!(decompressed, input, "L19 LDM round-trip failed");
    }

    #[test]
    fn ldm_improves_ratio_on_cross_block_repetition() {
        // Same 4 KiB block repeated 64 times = 256 KiB (2 blocks).
        // LDM should find cross-block matches that the normal
        // hash-table (cleared per block) would miss.
        let pattern: Vec<u8> = (0..4096u32)
            .map(|i| ((i.wrapping_mul(2654435761) >> 16) & 0xFF) as u8)
            .collect();
        let input: Vec<u8> = pattern.repeat(64); // 256 KiB

        // L18 doesn't use LDM (window_log < 19 threshold for Btultra2
        // strategy — L18 is Btultra, not Btultra2). L19+ uses LDM.
        let l18 = encode_frame_compressed(&input, 18).expect("L18");
        let l19 = encode_frame_compressed(&input, 19).expect("L19");
        let l22 = encode_frame_compressed(&input, 22).expect("L22");

        // All must round-trip.
        let d18 = decompress(&l18, input.len() as u32).expect("decode L18");
        let d19 = decompress(&l19, input.len() as u32).expect("decode L19");
        let d22 = decompress(&l22, input.len() as u32).expect("decode L22");
        assert_eq!(d18, input);
        assert_eq!(d19, input);
        assert_eq!(d22, input);

        // L19 should be smaller than L18 on cross-block repetition.
        // (LDM finds matches spanning block boundaries that L18 misses.)
        assert!(
            l19.len() <= l18.len(),
            "L19 ({}) should be ≤ L18 ({}) with LDM",
            l19.len(),
            l18.len()
        );
    }

    #[test]
    fn ldm_is_deterministic() {
        let pattern: Vec<u8> = (0..4096u32)
            .map(|i| ((i.wrapping_mul(2654435761) >> 16) & 0xFF) as u8)
            .collect();
        let input: Vec<u8> = pattern.repeat(64); // 256 KiB

        let a = encode_frame_compressed(&input, 19).expect("encode A");
        let b = encode_frame_compressed(&input, 19).expect("encode B");
        assert_eq!(a, b, "LDM encoder must be deterministic");
    }
}
