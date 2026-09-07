//! Reference-shaped greedy/lazy/lazy2 parse — a port of
//! `ZSTD_compressBlock_lazy_generic` (`zstd_lazy.c`, hashChain search,
//! noDict mode) including `ZSTD_HcFindBestMatch` and
//! `ZSTD_insertAndFindFirstIndex_internal`.
//!
//! This is the parse shape the reference runs at levels 5-12
//! (greedy/lazy/lazy2). The DP-shaped opt parser stays for the
//! btopt-and-up levels; the legacy block-local lazy parsers stay for
//! the dictionary path.
//!
//! Positions are ABSOLUTE stream positions (the caller passes
//! `src = &stream[..block_end]`), so hash and chain tables persist
//! across blocks exactly like the C match state.

#![forbid(unsafe_code)]

use crate::encoder::cparams::CompressionParams;
use crate::encoder::match_finder::{MatchState, RawSequence, SeqStore};

/// `REPCODE1_TO_OFFBASE` — offBase slot for "rep0" (zstd offBase
/// encoding: repcodes are 1..=3, real offsets are offset + 3).
const REPCODE1_TO_OFFBASE: u32 = 1;
/// `ZSTD_REP_NUM`.
const REP_NUM: usize = 3;
/// `kSearchStrength` (`zstd_compress_internal.h`).
const K_SEARCH_STRENGTH: usize = 8;
/// `kLazySkippingStep` (`zstd_lazy.c`).
const K_LAZY_SKIPPING_STEP: usize = 8;

const PRIME4_BYTES: u32 = 2_654_435_761;
const PRIME5_BYTES: u64 = 0xCF1B_BCDC_B7A5_6463;
const PRIME6_BYTES: u64 = 0x22FE_FFC9_944C_8DDD;

fn read32(src: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes([src[pos], src[pos + 1], src[pos + 2], src[pos + 3]])
}

fn read64(src: &[u8], pos: usize) -> u64 {
    u64::from_le_bytes([
        src[pos],
        src[pos + 1],
        src[pos + 2],
        src[pos + 3],
        src[pos + 4],
        src[pos + 5],
        src[pos + 6],
        src[pos + 7],
    ])
}

/// `ZSTD_hashPtr` for mls = 4/5/6. Reading 8 bytes at `pos` is safe:
/// hash positions are always < ilimit = iend - 8.
fn hash_ptr(src: &[u8], pos: usize, h_bits: u32, mls: usize) -> usize {
    let h = match mls {
        4 => u64::from(read32(src, pos).wrapping_mul(PRIME4_BYTES)) >> (32 - h_bits),
        6 => (read64(src, pos) << 16).wrapping_mul(PRIME6_BYTES) >> (64 - h_bits),
        // default: 5
        _ => (read64(src, pos) << 24).wrapping_mul(PRIME5_BYTES) >> (64 - h_bits),
    };
    h as usize
}

/// `ZSTD_count`: bytes in common between `[ip..iend)` and
/// `[match_pos..)`, word-at-a-time.
fn count(src: &[u8], ip: usize, match_pos: usize, iend: usize) -> usize {
    let limit = iend - ip;
    let mut len = 0usize;
    while len + 8 <= limit {
        let wa = read64(src, ip + len);
        let wb = read64(src, match_pos + len);
        if wa == wb {
            len += 8;
        } else {
            let trailing = (wa ^ wb).trailing_zeros() as usize;
            return len + trailing / 8;
        }
    }
    while len < limit && src[ip + len] == src[match_pos + len] {
        len += 1;
    }
    len
}

fn highbit32(v: u32) -> i32 {
    31 - v.leading_zeros() as i32
}

/// `ZSTD_insertAndFindFirstIndex_internal`: catch `next_to_update`
/// up to `target`, inserting each position into the hash + chain
/// tables, then return the hash-table head for `target`.
fn insert_and_find_first_index(
    src: &[u8],
    ms: &mut MatchState,
    target: usize,
    mls: usize,
    lazy_skipping: bool,
) -> u32 {
    let mut idx = ms.next_to_update as usize;
    while idx < target {
        let h = hash_ptr(src, idx, ms.hash_log, mls);
        ms.chain_table[idx & ms.chain_mask] = ms.hash_table[h];
        ms.hash_table[h] = idx as u32;
        idx += 1;
        if lazy_skipping {
            break;
        }
    }
    ms.next_to_update = target as u32;
    ms.hash_table[hash_ptr(src, target, ms.hash_log, mls)]
}

/// `ZSTD_HcFindBestMatch` (`noDict`): walk the hash chain from the
/// current head, keep the longest strictly-better match. Returns
/// `(match_length, off_base)` where off_base is `offset + 3` for a
/// real offset (0 when no candidate was long enough; the caller
/// gates on `ml >= 4` before using it).
fn hc_find_best_match(
    src: &[u8],
    ms: &mut MatchState,
    ip: usize,
    iend: usize,
    mls: usize,
    params: &CompressionParams,
) -> (usize, u32) {
    let chain_mask = ms.chain_mask;
    let chain_size = chain_mask + 1;
    let curr = ip as u32;
    let max_distance = 1u32 << params.window_log;
    // noDict: `lowestValid` is the stream start (position 0).
    let low_limit = if curr > max_distance {
        curr - max_distance
    } else {
        0
    };
    let min_chain = if (curr as usize) > chain_size {
        curr - chain_size as u32
    } else {
        0
    };
    let mut nb_attempts = 1u32 << params.search_log;
    let mut ml = 4usize - 1;
    let mut off_base = 0u32;

    let mut match_index = insert_and_find_first_index(src, ms, ip, mls, ms.lazy_skipping);

    while match_index >= low_limit && nb_attempts > 0 {
        let m = match_index as usize;
        let mut current_ml = 0usize;
        // Quick reject: compare the 4 bytes at match+ml-3. The
        // `ip+ml == iend` break below keeps this read in bounds the
        // same way the C's control flow does; the explicit bound
        // check covers the ml==3 initial value.
        if ip + ml < iend && read32(src, m + ml - 3) == read32(src, ip + ml - 3) {
            current_ml = 4 + count(src, ip + 4, m + 4, iend);
        }

        if current_ml > ml {
            ml = current_ml;
            off_base = curr - match_index + REP_NUM as u32;
            if ip + current_ml == iend {
                break;
            }
        }

        if match_index <= min_chain {
            break;
        }
        match_index = ms.chain_table[(match_index & chain_mask as u32) as usize];
        nb_attempts -= 1;
    }

    (ml, off_base)
}

/// `ZSTD_storeSeq` equivalent: literals `[lit_start..start)`, then
/// the sequence. `off_base` follows the C encoding (repcode 1, or
/// real offset + 3); the stored `offset` field is always the real
/// byte distance.
fn store_seq(
    src: &[u8],
    seq_store: &mut SeqStore,
    lit_start: usize,
    start: usize,
    off_base: u32,
    match_length: usize,
    offset_1: u32,
) {
    let real_offset = if off_base == REPCODE1_TO_OFFBASE {
        offset_1
    } else {
        off_base - REP_NUM as u32
    };
    seq_store.literals.extend_from_slice(&src[lit_start..start]);
    seq_store.sequences.push(RawSequence {
        literal_length: (start - lit_start) as u32,
        match_length: match_length as u32,
        offset: real_offset,
    });
}

/// `ZSTD_compressBlock_lazy_generic` (`noDict`, hashChain search).
/// `depth` = 0 (greedy), 1 (lazy), 2 (lazy2).
///
/// `src` is the whole stream prefix ending at the block end;
/// `block_start` is the first position of this block. Rep state
/// comes in via `seq_store.rep_offsets`; the parser mirrors the
/// decoder's rep evolution (rotate on real offsets, swap on
/// zero-literal rep sequences) so the real offsets it emits stay
/// rep-codable.
#[allow(clippy::too_many_lines)]
pub fn compress_block_lazy_generic(
    src: &[u8],
    block_start: usize,
    seq_store: &mut SeqStore,
    ms: &mut MatchState,
    params: &CompressionParams,
    depth: u32,
) -> usize {
    let iend = src.len();
    let mls = params.min_match.clamp(4, 6) as usize;
    let ilimit = iend.saturating_sub(8);
    let max_distance = 1usize << params.window_log;

    let mut offset_1 = seq_store.rep_offsets[0];
    let mut offset_2 = seq_store.rep_offsets[1];
    let mut offset_saved1 = 0u32;
    let mut offset_saved2 = 0u32;

    let mut anchor = block_start;
    let mut ip = block_start + usize::from(block_start == 0);

    if ip >= ilimit {
        seq_store
            .literals
            .extend_from_slice(&src[block_start..iend]);
        return iend - block_start;
    }

    // noDict rep clamping: an offset older than the window is unusable.
    let max_rep = ip - ip.saturating_sub(max_distance);
    if offset_2 as usize > max_rep {
        offset_saved2 = offset_2;
        offset_2 = 0;
    }
    if offset_1 as usize > max_rep {
        offset_saved1 = offset_1;
        offset_1 = 0;
    }

    while ip < ilimit {
        let mut match_length = 0usize;
        let mut off_base = REPCODE1_TO_OFFBASE;
        let mut start = ip + 1;

        // check repcode (at ip+1): MEM_read32(ip+1-offset_1)
        if offset_1 > 0 {
            if let Some(rp) = (ip + 1).checked_sub(offset_1 as usize) {
                if read32(src, rp) == read32(src, ip + 1) {
                    match_length = 4 + count(src, ip + 5, rp + 4, iend);
                    if depth == 0 {
                        // greedy: store immediately
                        store_seq(
                            src,
                            seq_store,
                            anchor,
                            start,
                            off_base,
                            match_length,
                            offset_1,
                        );
                        anchor = start + match_length;
                        ip = anchor;
                        ms.lazy_skipping = false;
                        immediate_repcode(
                            src,
                            seq_store,
                            &mut ip,
                            &mut anchor,
                            &mut offset_1,
                            &mut offset_2,
                            ilimit,
                            iend,
                        );
                        continue;
                    }
                }
            }
        }

        // first search
        let (ml2, ofb) = hc_find_best_match(src, ms, ip, iend, mls, params);
        if ml2 > match_length {
            match_length = ml2;
            start = ip;
            off_base = ofb;
        }

        if match_length < 4 {
            let step = ((ip - anchor) >> K_SEARCH_STRENGTH) + 1;
            ip += step;
            ms.lazy_skipping = step > K_LAZY_SKIPPING_STEP;
            continue;
        }

        // try to find a better solution (depth 1 and 2 lookahead)
        if depth >= 1 {
            while ip < ilimit {
                ip += 1;
                // rep at ip
                if offset_1 > 0 {
                    if let Some(rp) = ip.checked_sub(offset_1 as usize) {
                        if read32(src, rp) == read32(src, ip) {
                            let ml_rep = 4 + count(src, ip + 4, rp + 4, iend);
                            let gain2 = (ml_rep * 3) as i32;
                            let gain1 = (match_length * 3) as i32 - highbit32(off_base) + 1;
                            if ml_rep >= 4 && gain2 > gain1 {
                                match_length = ml_rep;
                                off_base = REPCODE1_TO_OFFBASE;
                                start = ip;
                            }
                        }
                    }
                }
                // search at ip
                let (ml2, ofb) = hc_find_best_match(src, ms, ip, iend, mls, params);
                let gain2 = (ml2 * 4) as i32 - highbit32(ofb.max(1));
                let gain1 = (match_length * 4) as i32 - highbit32(off_base) + 4;
                if ml2 >= 4 && gain2 > gain1 {
                    match_length = ml2;
                    off_base = ofb;
                    start = ip;
                    continue; // search an even better one
                }

                // depth 2: one more lookahead round
                if depth == 2 && ip < ilimit {
                    ip += 1;
                    if offset_1 > 0 {
                        if let Some(rp) = ip.checked_sub(offset_1 as usize) {
                            if read32(src, rp) == read32(src, ip) {
                                let ml_rep = 4 + count(src, ip + 4, rp + 4, iend);
                                let gain2 = (ml_rep * 4) as i32;
                                let gain1 = (match_length * 4) as i32 - highbit32(off_base) + 1;
                                if ml_rep >= 4 && gain2 > gain1 {
                                    match_length = ml_rep;
                                    off_base = REPCODE1_TO_OFFBASE;
                                    start = ip;
                                }
                            }
                        }
                    }
                    let (ml2, ofb) = hc_find_best_match(src, ms, ip, iend, mls, params);
                    let gain2 = (ml2 * 4) as i32 - highbit32(ofb.max(1));
                    let gain1 = (match_length * 4) as i32 - highbit32(off_base) + 7;
                    if ml2 >= 4 && gain2 > gain1 {
                        match_length = ml2;
                        off_base = ofb;
                        start = ip;
                        continue;
                    }
                }
                break;
            }
        }

        // catch up: extend the match backwards over literals
        if off_base > REP_NUM as u32 {
            let offset = (off_base - REP_NUM as u32) as usize;
            while start > anchor && start - offset > 0 && src[start - 1] == src[start - 1 - offset]
            {
                start -= 1;
                match_length += 1;
            }
            offset_2 = offset_1;
            offset_1 = offset as u32;
        }

        // store sequence
        store_seq(
            src,
            seq_store,
            anchor,
            start,
            off_base,
            match_length,
            offset_1,
        );
        anchor = start + match_length;
        ip = anchor;
        ms.lazy_skipping = false;

        // check immediate repcode (offset_2)
        immediate_repcode(
            src,
            seq_store,
            &mut ip,
            &mut anchor,
            &mut offset_1,
            &mut offset_2,
            ilimit,
            iend,
        );
    }

    // save reps for next block
    seq_store.rep_offsets[0] = if offset_1 > 0 {
        offset_1
    } else {
        offset_saved1
    };
    seq_store.rep_offsets[1] = if offset_2 > 0 {
        offset_2
    } else {
        offset_saved2
    };
    seq_store.rep_offsets[2] = 8;

    if anchor < iend {
        seq_store.literals.extend_from_slice(&src[anchor..iend]);
    }
    iend - anchor
}

/// The post-store `offset_2` repcode loop (noDict): while the bytes
/// at `ip` equal the bytes at `ip - offset_2`, store a zero-literal
/// rep sequence and swap the two reps.
#[allow(clippy::too_many_arguments)]
fn immediate_repcode(
    src: &[u8],
    seq_store: &mut SeqStore,
    ip: &mut usize,
    anchor: &mut usize,
    offset_1: &mut u32,
    offset_2: &mut u32,
    ilimit: usize,
    iend: usize,
) {
    while *ip <= ilimit && *offset_2 > 0 {
        let rp = match ip.checked_sub(*offset_2 as usize) {
            Some(rp) if rp + 4 <= iend && *ip + 4 <= iend => rp,
            _ => break,
        };
        if read32(src, rp) != read32(src, *ip) {
            break;
        }
        let match_length = 4 + count(src, *ip + 4, rp + 4, iend);
        let used = *offset_2;
        std::mem::swap(offset_1, offset_2);
        seq_store.sequences.push(RawSequence {
            literal_length: 0,
            match_length: match_length as u32,
            offset: used,
        });
        *ip += match_length;
        *anchor = *ip;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hash_ptr_stays_in_range() {
        let src = b"abcdefgh12345678";
        for pos in 0..8 {
            for mls in [4usize, 5, 6] {
                assert!(super::hash_ptr(src, pos, 12, mls) < (1 << 12));
            }
        }
    }

    fn roundtrip(data: &[u8], level: u8) {
        let z = crate::encoder::block::encode_frame_compressed(data, level).unwrap();
        let back = crate::decompress(&z, 0).unwrap();
        assert_eq!(back, data, "level {level}");
    }

    #[test]
    fn lazy_parses_roundtrip() {
        for level in [5u8, 6, 7, 8, 12] {
            let data = b"hello hello hello hello world world world 1234567890".repeat(20);
            roundtrip(&data, level);
        }
    }

    #[test]
    fn lazy_handles_all_zero_and_periodic() {
        // Bounded-work fixtures (CLAUDE.md Invariant 1).
        for level in [5u8, 6, 12] {
            roundtrip(&vec![0u8; 300_000], level);

            let period: Vec<u8> = (0..7000u32).map(|i| (i % 251) as u8).collect();
            let periodic = period.repeat(50);
            roundtrip(&periodic, level);
        }
    }
}
