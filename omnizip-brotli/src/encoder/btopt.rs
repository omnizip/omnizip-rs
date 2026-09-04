//! btopt — optimal parsing for the quality 10-11 tier.
//!
//! Net-new for the brotli encoder (absent from both the C reference
//! and the Ruby original): a generalization of the LZMA optimum
//! parser (omnizip-lzma/src/encoder/optimum.rs, xz's
//! `lzma_encoder_optimum_normal.c`) shaped like zstd's btopt — a
//! per-position shortest-path DP in which every node carries the
//! FULL encoder rep-ring state, maintained incrementally (the PR #283
//! `rep_state[]` pattern: the optimal path into `i` is frozen before
//! `i` is processed, so the ring derives in O(1) from the chosen
//! transition instead of walking the backpointer chain).
//!
//! Candidate set per position (the LZMA helper1/helper2 enumeration,
//! mapped onto brotli's fused insert+copy commands):
//!
//! - literal step (positional, context-partitioned bit cost);
//! - the 16 distance short codes probed against the node's exact
//!   ring (rep0-3 exact, rep0/rep1 ±1-3) — each priced at its true
//!   wire shape: implicit-rep0 command folding when the symbol
//!   allows it, else a short-code distance symbol;
//! - every H10 match-list entry (shared collection with
//!   [`zopfli_hq`], including the q11 short-match scan), swept at all
//!   lengths with the `MaxZopfliLen` long-copy jump;
//! - static-dictionary references (identity LUT probe) as first-class
//!   transitions — the q10/11 tier had none.
//!
//! Two passes: pass 0 rides the sliding-window literal model; pass 1
//! re-prices commands/distances from pass 0's histogram (rep-code
//! feedback — the round-22 lesson) and literals from per-context
//! `SetCost` tables. The parse is a CANDIDATE for the exact-emission
//! contest in `from_spec_encoder`: it ships only when its measured
//! metablock is smaller than the reference zopfli port's, so the
//! tier's output can only improve.
//!
//! Determinism: a pure function of (input, quality) — no clocks, no
//! unordered containers, single-threaded, fixed float evaluation
//! order.

// Port-typical shape (see omnizip-lzma/src/encoder/optimum.rs): index
// arithmetic with deliberate narrowing casts and LZMA-style terse
// loop variables, kept diffable against the reference structure.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::similar_names,
    clippy::too_many_arguments
)]

use crate::encoder::context::{compute_context_id, is_text_like};
use crate::encoder::zopfli_hq::{
    collect_matches, compute_minimum_copy_length, long_dist_symbol, set_cost, CostModel,
};
use crate::from_spec_encoder::{find_cmd_symbol, find_cmd_symbol_with_rep, Command};
use crate::prefix::kCmdLut;

const K_INFINITY: f32 = 1.7e38;
/// Upstream caps DP copy lengths at 1951 (`kMaxMatchLen` bucketing in
/// `UpdateNodes`).
/// Upstream's kMaxMatchLen-class cap. The bucket-boundary stepping
/// does NOT fully bound the sweep on repetitive content: LimniFS
/// issue #388 — raising this to 65_536 hung CI on windows-latest
/// (23+ min on tens-of-KB repetitive structured text where 1951
/// completes in ms). The ~115-byte ratio gain on binary data does
/// not justify the hang risk; revert to 1951.
const MATCH_LEN_CAP: usize = 1951;
/// `MAX_ZOPFLI_LEN_QUALITY_10` / `_11`.
const MAX_ZOPFLI_LEN: [usize; 2] = [150, 325];

/// Back-pointer encoding: 0..=15 = distance short code (0 = rep0,
/// ring unchanged), 16 = long-form push, 17 = dictionary (ring
/// unchanged).
const CODE_LONG: u8 = 16;
const CODE_DICT: u8 = 17;

/// Short-code index/offset tables for the 16 RFC 7932 distance codes
/// (upstream `kDistanceCacheIndex` / `kDistanceCacheOffset`). Applied
/// against the newest-first ring [rep0, rep1, rep2, rep3].
const CACHE_INDEX: [usize; 16] = [0, 1, 2, 3, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1];
const CACHE_OFFSET: [i32; 16] = [0, 0, 0, 0, -1, 1, -2, 2, -3, 3, -1, 1, -2, 2, -3, 3];

/// Fresh-chunk rep ring in newest-first order — `RepBuffer::new()`
/// (`dist_rb = [16, 15, 11, 4]`, `idx = 0`) read through `rep_at`,
/// which yields rep0 = 4, rep1 = 11, rep2 = 15, rep3 = 16.
const INITIAL_RING: [u32; 4] = [4, 11, 15, 16];

/// Push a used distance onto the newest-first ring. Codes 1-15 and
/// the long form all resolve to a push (explicit code 0 and the
/// implicit fold are ring no-ops) — exactly `RepBuffer`'s update
/// block in `build_symbol_stream`.
#[inline]
fn push_ring(ring: &[u32; 4], d: u32) -> [u32; 4] {
    [d, ring[0], ring[1], ring[2]]
}

/// Short-code delta order for codes 4-9 (rep0 ± d) and 10-15 (rep1 ±
/// d), matching `RepBuffer::find_short_code`.
const SHORT_CODE_DELTAS: [i32; 6] = [-1, 1, -2, 2, -3, 3];

/// Cheapest distance short code reproducing `dist` from `ring`
/// (mirror of `RepBuffer::find_short_code`'s fixed search order).
#[inline]
fn short_code_for(dist: u32, ring: &[u32; 4]) -> Option<u32> {
    for code in 0..4u32 {
        if ring[code as usize] == dist {
            return Some(code);
        }
    }
    let rep0 = ring[0] as i32;
    for (k, &d) in SHORT_CODE_DELTAS.iter().enumerate() {
        if rep0 + d == dist as i32 && rep0 + d >= 1 {
            return Some(4 + k as u32);
        }
    }
    let rep1 = ring[1] as i32;
    for (k, &d) in SHORT_CODE_DELTAS.iter().enumerate() {
        if rep1 + d == dist as i32 && rep1 + d >= 1 {
            return Some(10 + k as u32);
        }
    }
    None
}

/// Context-partitioned per-position literal costs (pass 1): `SetCost`
/// tables per (p1, p2) context over the whole input, mode matching
/// the emission's (`UTF8` for text, `LSB6` otherwise). The context of
/// a byte depends only on its position, so the table is
/// parse-independent and deterministic.
fn ctx_literal_costs(input: &[u8]) -> Vec<f32> {
    let n = input.len();
    let mode: u32 = if is_text_like(input) { 2 } else { 0 };
    let ctx_of = |i: usize| -> usize {
        let p1 = if i >= 1 { input[i - 1] } else { 0 };
        let p2 = if i >= 2 { input[i - 2] } else { 0 };
        compute_context_id(p1, p2, mode) as usize
    };
    let mut hists = vec![[0u32; 256]; 64];
    for i in 0..n {
        hists[ctx_of(i)][usize::from(input[i])] += 1;
    }
    let mut tables = vec![[0.0f32; 256]; 64];
    for ctx in 0..64 {
        set_cost(&hists[ctx], true, &mut tables[ctx]);
    }
    (0..n)
        .map(|i| tables[ctx_of(i)][usize::from(input[i])])
        .collect()
}

/// Static-dictionary candidate per position: `(distance, word_len,
/// transformed_len)`, with the distance computed against the decoder's
/// clamped output position so it always classifies as a dictionary
/// reference. Probed only where the H10 match list's best length is
/// below 16 (the `zopfli_collect` gate): the LUT probe is ~30% of
/// parse time when run at every position, and a position already
/// covered by a long match rarely prefers a dict word.
pub(crate) fn build_dict_at(
    input: &[u8],
    mlen_offset: usize,
    offsets: &[u32],
    num_matches: &[u32],
    matches: &[(u32, u32)],
) -> Vec<Option<(u32, u32, u32)>> {
    let n = input.len();
    let mut dict_at = vec![None; n];
    for pos in 0..n {
        let best_len = if num_matches[pos] > 0 {
            matches[(offsets[pos] + num_matches[pos] - 1) as usize].1
        } else {
            0
        };
        if best_len >= 16 {
            continue;
        }
        let global_pos = mlen_offset + pos;
        let max_dist = (global_pos as u32).min(crate::from_spec_encoder::MAX_BACKWARD_DISTANCE);
        if let Some((d, wl, _finder_tl)) =
            crate::encoder::dict_hash::find_match(input, pos, max_dist)
        {
            // The transformed length that actually reaches the output
            // is dictionary_lookup's (the decoder's view). The finder's
            // own tl can disagree on transform selection — trusting it
            // drifts every later command's position (the arial q10
            // chunk-overrun panic: a +10-byte cumulative shift).
            let mut tmp = Vec::new();
            if crate::dictionary::dictionary_lookup(&mut tmp, wl, d as i32, max_dist) == Some(())
                && tmp.len() >= 2
                && pos + tmp.len() <= n
            {
                let tl = tmp.len() as u32;
                dict_at[pos] = Some((d, wl, tl));
            }
        }
    }
    dict_at
}

/// DP arrays (one node per input position, LZMA `opts[]` shape).
struct DpState {
    cost: Vec<f32>,
    /// Start of the pending literal run (LZMA `u[]`).
    u: Vec<u32>,
    back_pos: Vec<u32>,
    /// Advance of the copy transition into the node (dict: transformed
    /// length; LZ77: copy length). 0 = literal step.
    back_len: Vec<u32>,
    back_dist: Vec<u32>,
    /// 0..=15 short code, [`CODE_LONG`], or [`CODE_DICT`].
    back_code: Vec<u8>,
    /// Rep ring AFTER the transition into the node (newest-first).
    ring: Vec<[u32; 4]>,
}

impl DpState {
    fn new(n: usize) -> Self {
        Self {
            cost: vec![K_INFINITY; n + 1],
            u: vec![0; n + 1],
            back_pos: vec![0; n + 1],
            back_len: vec![0; n + 1],
            back_dist: vec![0; n + 1],
            back_code: vec![0; n + 1],
            ring: vec![INITIAL_RING; n + 1],
        }
    }

    #[inline]
    fn relax(
        &mut self,
        j: usize,
        i: usize,
        advance: u32,
        dist: u32,
        code: u8,
        total: f32,
        next_ring: [u32; 4],
    ) {
        if total < self.cost[j] {
            self.cost[j] = total;
            self.back_pos[j] = i as u32;
            self.back_len[j] = advance;
            self.back_dist[j] = dist;
            self.back_code[j] = code;
            self.u[j] = j as u32;
            self.ring[j] = next_ring;
        }
    }

    #[inline]
    fn relax_literal(&mut self, j: usize, c: f32) {
        if c < self.cost[j] {
            self.cost[j] = c;
            self.back_len[j] = 0;
            self.u[j] = self.u[j - 1];
            self.ring[j] = self.ring[j - 1];
        }
    }
}

/// One DP pass over `input`. Produces the optimal chain's commands.
#[allow(clippy::too_many_lines)]
fn dp_pass(
    input: &[u8],
    quality: i32,
    lit_pos: &[f32],
    model: &CostModel,
    dict_at: &[Option<(u32, u32, u32)>],
    offsets: &[u32],
    num_matches: &[u32],
    matches: &[(u32, u32)],
) -> Vec<Command> {
    let n = input.len();
    let tier = usize::from(quality >= 11);
    let mut probe_bytes: u64 = 0;
    let probe_t1 = (n as u64) * 8_192;
    let probe_t2 = (n as u64) * 16_384;
    let max_z = MAX_ZOPFLI_LEN[tier];
    let cmd_table = &model.cost_cmd;
    let dist_table = &model.cost_dist;
    let mut dp = DpState::new(n);
    dp.cost[0] = 0.0;

    for i in 0..n {
        let base = dp.cost[i];

        // Literal step.
        let c = base + lit_pos[i];
        dp.relax_literal(i + 1, c);

        let reps = dp.ring[i];
        let ilen = (i - dp.u[i] as usize) as u32;

        let min_len = compute_minimum_copy_length(base + model.min_cost_cmd, |p| dp.cost[p], n, i);

        // --- static-dictionary candidate ---
        if let Some((d, wl, tl)) = dict_at[i] {
            if let Some(sym) = find_cmd_symbol(ilen, wl) {
                let e = &kCmdLut[sym];
                let dsym = long_dist_symbol(d);
                let extra = (((dsym as u32) - 16) >> 1) + 1;
                let total = base
                    + cmd_table[sym]
                    + f32::from(e.insert_len_extra_bits)
                    + f32::from(e.copy_len_extra_bits)
                    + dist_table[dsym]
                    + extra as f32;
                let j = i + tl as usize;
                if j <= n {
                    dp.relax(j, i, tl, d, CODE_DICT, total, reps);
                }
            }
        }

        // --- 16 short-code rep candidates (LZMA helper1's rep block).
        // Probe lengths are capped like the match-list sweep: without
        // the cap, long repetitive runs (all-zeros) sweep O(remaining)
        // relaxations per position — the task-#312 quadratic class. ---
        let mut seen = [0u32; 16];
        let mut seen_n = 0usize;
        let mut best_len = min_len.saturating_sub(1);
        // Work budget (issues #388/#408 class, #312 pathology): the
        // probe compares are the dominant cost on content where all
        // 16 rep distances match maximally (all-zeros measures ~47K
        // compare bytes per position — every probe runs its full cap
        // at every position, even interior to monster copies). Normal
        // content stays far below the first threshold (bin1, the
        // heaviest measured, ~1.5K per position; the 8,192 stage
        // sits 5.5x above it), so the parse is byte-identical there;
        // pathological content degrades to a hard-bounded ceiling.
        let probe_cap = (n - i).min(if probe_bytes >= probe_t2 {
            64
        } else if probe_bytes >= probe_t1 {
            256
        } else {
            MATCH_LEN_CAP
        });
        for jcode in 0..16usize {
            let raw = i64::from(reps[CACHE_INDEX[jcode]]) + i64::from(CACHE_OFFSET[jcode]);
            let dist = if raw >= 1 && raw <= i as i64 {
                raw as u32
            } else {
                continue;
            };
            if seen[..seen_n].contains(&dist) {
                continue;
            }
            seen[seen_n] = dist;
            seen_n += 1;
            let prev = i - dist as usize;
            // Quick reject before the compare (upstream EvaluateNode).
            if i + best_len < n && input[i + best_len] != input[prev + best_len] {
                continue;
            }
            let mut len = 0usize;
            while i + len < n && len < probe_cap && input[i + len] == input[prev + len] {
                len += 1;
            }
            crate::encoder::work_meter::add(3, len as u64);
            probe_bytes += len as u64;
            if len > best_len {
                sweep_lengths((best_len + 1).max(2), len, |l2| {
                    relax_copy(&mut dp, i, l2, dist, ilen, &reps, cmd_table, dist_table);
                });
                best_len = len;
            }
        }

        // --- H10 match list (normal distance codes) ---
        let cstart = offsets[i] as usize;
        let cend = cstart + num_matches[i] as usize;
        let mut len = min_len;
        for &(dist, mlen) in &matches[cstart..cend] {
            // Static-dictionary candidates from collect_matches arrive
            // in the shared match list with distances beyond the
            // window. Pricing them as huge real distances overpays;
            // route through the CODE_DICT relax (and only when
            // dict_at recorded the entry — the backtrack resolves
            // through it).
            if u64::from(dist) > crate::from_spec_encoder::MAX_BACKWARD_DISTANCE as u64 {
                if let Some((d, wl, _tl)) = dict_at[i] {
                    if d == dist {
                        if let Some(sym) = find_cmd_symbol(ilen, wl) {
                            let e = &kCmdLut[sym];
                            let dsym = long_dist_symbol(d);
                            let extra = (((dsym as u32) - 16) >> 1) + 1;
                            let total = base
                                + cmd_table[sym]
                                + f32::from(e.insert_len_extra_bits)
                                + f32::from(e.copy_len_extra_bits)
                                + dist_table[dsym]
                                + extra as f32;
                            let j = i + wl as usize;
                            if j <= n {
                                dp.relax(j, i, wl, d, CODE_DICT, total, reps);
                            }
                        }
                    }
                }
                continue;
            }
            let maxlen = (mlen as usize).min(n - i).min(MATCH_LEN_CAP);
            if len < maxlen && maxlen > max_z {
                len = maxlen;
            }
            sweep_lengths(len.max(2), maxlen, |l| {
                relax_copy(&mut dp, i, l, dist, ilen, &reps, cmd_table, dist_table);
            });
            len = maxlen + 1;
            // Whole-match jump (see zopfli_hq): one relaxation at the
            // full finder length when it exceeds the DP cap. Constant
            // work per candidate; the capped sweep is untouched.
            let full_len = (mlen as usize).min(n - i);
            if full_len > maxlen {
                relax_copy(
                    &mut dp, i, full_len, dist, ilen, &reps, cmd_table, dist_table,
                );
            }
        }
    }

    backtrack(&dp, input, dict_at)
}

/// Copy lengths above this are swept only at copy-code bucket
/// boundaries (plus the candidate's own max): within a bucket the
/// transition cost is constant, so intermediate lengths are
/// redundant — and sweeping every length up to the 1951 cap at every
/// position is the task-#312 quadratic class on repetitive input.
const SWEEP_FULL_CAP: usize = 128;
/// Copy-code bucket starts above [`SWEEP_FULL_CAP`] (kCopyBase
/// offsets, up to the 1951 DP cap).
const SWEEP_BOUNDARIES: [usize; 5] = [134, 198, 326, 582, 1094];

#[inline]
fn sweep_lengths<F: FnMut(usize)>(lo: usize, hi: usize, mut f: F) {
    crate::encoder::work_meter::add(4, 1);
    if lo > hi {
        return;
    }
    let mut l = lo;
    while l < hi {
        f(l);
        l = if l < SWEEP_FULL_CAP {
            l + 1
        } else {
            SWEEP_BOUNDARIES
                .iter()
                .find(|&&b| b > l)
                .copied()
                .unwrap_or(hi)
                .min(hi)
        };
    }
    f(hi);
}

/// Price one LZ77 copy `(i, len, dist)` at its exact wire shape and
/// relax the target node.
#[allow(clippy::too_many_arguments)]
fn relax_copy(
    dp: &mut DpState,
    i: usize,
    len: usize,
    dist: u32,
    ilen: u32,
    reps: &[u32; 4],
    cmd_table: &[f32],
    dist_table: &[f32],
) {
    let base = dp.cost[i];
    let j = i + len;

    // Implicit-rep0 fold: the command symbol implies "use last
    // distance" — no distance symbol at all. Mirrors
    // build_symbol_stream's `can_use_implicit`.
    if dist == reps[0] {
        if let Some(sym) = find_cmd_symbol_with_rep(ilen, len as u32, Some(0)) {
            if kCmdLut[sym].distance_code == 0 {
                let e = &kCmdLut[sym];
                let total = base
                    + cmd_table[sym]
                    + f32::from(e.insert_len_extra_bits)
                    + f32::from(e.copy_len_extra_bits);
                dp.relax(j, i, len as u32, dist, 0, total, *reps);
                return;
            }
        }
    }

    // Explicit command symbol + distance encoding.
    let Some(sym) = find_cmd_symbol(ilen, len as u32) else {
        return;
    };
    let e = &kCmdLut[sym];
    let cmd_cost =
        cmd_table[sym] + f32::from(e.insert_len_extra_bits) + f32::from(e.copy_len_extra_bits);

    if let Some(sc) = short_code_for(dist, reps) {
        let total = base + cmd_cost + dist_table[sc as usize];
        let next_ring = if sc == 0 {
            *reps
        } else {
            push_ring(reps, dist)
        };
        dp.relax(j, i, len as u32, dist, sc as u8, total, next_ring);
    } else {
        let dsym = long_dist_symbol(dist);
        let extra = (((dsym as u32) - 16) >> 1) + 1;
        let total = base + cmd_cost + dist_table[dsym] + extra as f32;
        dp.relax(
            j,
            i,
            len as u32,
            dist,
            CODE_LONG,
            total,
            push_ring(reps, dist),
        );
    }
}

/// Walk the backpointers forward into a command list, appending the
/// trailing-insert command for any uncovered literal tail.
fn backtrack(dp: &DpState, input: &[u8], dict_at: &[Option<(u32, u32, u32)>]) -> Vec<Command> {
    let n = input.len();
    let mut chain: Vec<Command> = Vec::new();
    let mut advances: Vec<u32> = Vec::new();
    let mut j = n;
    while j > 0 {
        if dp.back_len[j] == 0 {
            j -= 1;
            continue;
        }
        let i = dp.back_pos[j] as usize;
        let ins = (i - dp.u[i] as usize) as u32;
        if dp.back_code[j] == CODE_DICT {
            let (d, wl, _tl) = dict_at[i].expect("dict candidate recorded at copy start");
            chain.push(Command {
                insert_len: ins,
                copy_len: wl,
                distance: d,
            });
            advances.push(dp.back_len[j]);
        } else {
            chain.push(Command {
                insert_len: ins,
                copy_len: dp.back_len[j],
                distance: dp.back_dist[j],
            });
            advances.push(dp.back_len[j]);
        }
        j = i;
    }
    chain.reverse();
    advances.reverse();

    let mut covered = 0usize;
    for (cmd, &adv) in chain.iter().zip(&advances) {
        covered += cmd.insert_len as usize + adv as usize;
    }
    if covered < n {
        chain.push(Command {
            insert_len: (n - covered) as u32,
            copy_len: 0,
            distance: 0,
        });
    }
    chain
}

/// Parse with a shared H10 collection (the q10/11 routing collects
/// once and feeds both this parser and `zopfli_hq`).
pub(crate) fn parse_btopt_with(
    input: &[u8],
    quality: i32,
    mlen_offset: usize,
    num_matches: &[u32],
    matches: &[(u32, u32)],
) -> Vec<Command> {
    let n = input.len();
    if n < 8 {
        return Vec::new();
    }
    // Prefix-sum offsets into the flat match list.
    let mut offsets = vec![0u32; n + 1];
    for i in 0..n {
        offsets[i + 1] = offsets[i] + num_matches[i];
    }
    let dict_at = build_dict_at(input, mlen_offset, &offsets, num_matches, matches);
    let ctx_lits = ctx_literal_costs(input);

    let model0 = CostModel::from_literal_costs(input);
    let lit0: Vec<f32> = (0..n).map(|i| model0.lit_costs(i, i + 1)).collect();
    let pass0 = dp_pass(
        input,
        quality,
        &lit0,
        &model0,
        &dict_at,
        &offsets,
        num_matches,
        matches,
    );

    let model1 =
        CostModel::from_commands(input, &pass0, mlen_offset).with_positional_literals(&ctx_lits);
    dp_pass(
        input,
        quality,
        &ctx_lits,
        &model1,
        &dict_at,
        &offsets,
        num_matches,
        matches,
    )
}

/// Standalone entry (own collection) — tests and diagnostics.
#[must_use]
pub fn parse_btopt(input: &[u8], quality: i32) -> Vec<Command> {
    let n = input.len();
    if n < 8 {
        return Vec::new();
    }
    let mut tree = omnizip_codecs::BinaryTreeMatchFinder::new(input);
    let (num_matches, matches) = collect_matches(input, &mut tree, quality);
    parse_btopt_with(input, quality, 0, &num_matches, &matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walk a command list the way the emission does and assert exact
    /// coverage and distance legality.
    fn assert_valid(input: &[u8], cmds: &[Command]) {
        let n = input.len();
        let mut pos = 0usize;
        for (k, cmd) in cmds.iter().enumerate() {
            assert!(
                pos + cmd.insert_len as usize <= n,
                "cmd {k}: insert overrun at {pos}"
            );
            pos += cmd.insert_len as usize;
            if cmd.copy_len == 0 {
                assert!(k + 1 == cmds.len(), "copy_len 0 outside trailing command");
                continue;
            }
            let max_dist = (pos as u32).min(crate::from_spec_encoder::MAX_BACKWARD_DISTANCE);
            let is_dict = cmd.distance > max_dist;
            if is_dict {
                let mut scratch = Vec::new();
                assert!(
                    crate::dictionary::dictionary_lookup(
                        &mut scratch,
                        cmd.copy_len,
                        cmd.distance as i32,
                        max_dist
                    )
                    .is_some(),
                    "cmd {k}: unresolvable dict distance {}",
                    cmd.distance
                );
                pos += scratch.len();
            } else {
                assert!(cmd.distance >= 1, "cmd {k}: zero distance");
                assert!(
                    cmd.distance as usize <= pos,
                    "cmd {k}: distance {} exceeds history {pos}",
                    cmd.distance
                );
                assert!(cmd.copy_len >= 2, "cmd {k}: copy_len < 2");
                pos += cmd.copy_len as usize;
            }
        }
        assert_eq!(pos, n, "commands cover exactly the input");
    }

    #[test]
    fn covers_csv_like_input() {
        let input: Vec<u8> = (0..2000)
            .map(|i| format!("row_{},{},value_{},text here\n", i, i * 7, i % 13))
            .collect::<String>()
            .into_bytes();
        assert_valid(&input, &parse_btopt(&input, 11));
    }

    #[test]
    fn covers_binary_input() {
        let input: Vec<u8> = (0..8192u32)
            .map(|i| ((i.wrapping_mul(2654435761)) >> 13) as u8)
            .collect();
        assert_valid(&input, &parse_btopt(&input, 11));
    }

    #[test]
    fn covers_repetitive_input() {
        let input = vec![0xAB; 100_000];
        assert_valid(&input, &parse_btopt(&input, 10));
    }

    #[test]
    fn covers_dict_word_input() {
        // Static-dictionary words ("information", "representation")
        // make dict candidates reachable in the DP.
        let input =
            b"information representation information representation of the information ".repeat(20);
        assert_valid(&input, &parse_btopt(&input, 11));
    }

    #[test]
    fn deterministic() {
        let input: Vec<u8> = (0..4096u32)
            .map(|i| (i % 251 + (i / 251) % 17) as u8)
            .collect();
        let a = parse_btopt(&input, 11);
        let b = parse_btopt(&input, 11);
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(x, y)| x.insert_len == y.insert_len
            && x.copy_len == y.copy_len
            && x.distance == y.distance));
    }

    /// End-to-end determinism + round-trip through the q10/11 routing
    /// (the exact-emission contest and winner-reuse path).
    #[test]
    fn encode_deterministic_and_round_trips_q10_q11() {
        let input: Vec<u8> = (0..1500)
            .map(|i| format!("row {},{},value_{}\n", i, i * 3, i % 9))
            .collect::<String>()
            .into_bytes();
        for q in [10, 11] {
            let a = crate::from_spec_encoder::compress_with_quality(&input, q);
            let b = crate::from_spec_encoder::compress_with_quality(&input, q);
            assert_eq!(a, b, "q{q} encode must be byte-deterministic");
            let decoded = crate::decoder::decode(&a).expect("decode");
            assert_eq!(decoded, input, "q{q} round-trip");
        }
    }
}
