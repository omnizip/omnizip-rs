//! Reference-faithful port of brotli 1.2.0's HQ zopfli backward
//! references (`BrotliCreateHqZopfliBackwardReferences`, quality 11):
//! the `ZopfliNode` shortest-path DP with an 8-deep StartPosQueue of
//! alternative command-start positions, rep-code relaxation from each
//! start's reconstructed distance cache, and a two-iteration cost
//! model (pass 0: sliding-window literal costs; pass 1: histogram
//! costs derived from pass 0's commands).
//!
//! Sources (brotli 1.2.0): `backward_references_hq.c`, `literal_cost.c`,
//! `hash_to_binary_tree_inc.h`, `quality.h`.

use crate::from_spec_encoder::Command;
use crate::static_codes::{K_COPY_EXTRA, K_INS_EXTRA};

fn log2_floor_non_zero(v: u64) -> u32 {
    63 - v.leading_zeros()
}

/// RFC 7932 §10.3 insert-length code.
fn get_insert_length_code(insertlen: usize) -> u16 {
    if insertlen < 6 {
        insertlen as u16
    } else if insertlen < 130 {
        let nbits = log2_floor_non_zero((insertlen - 2) as u64) - 1;
        (((nbits << 1) as usize) + ((insertlen - 2) >> nbits) + 2) as u16
    } else if insertlen < 2114 {
        log2_floor_non_zero((insertlen - 66) as u64) as u16 + 10
    } else if insertlen < 6210 {
        21
    } else if insertlen < 22594 {
        22
    } else {
        23
    }
}

/// RFC 7932 §10.3 copy-length code.
fn get_copy_length_code(copylen: usize) -> u16 {
    if copylen < 10 {
        (copylen - 2) as u16
    } else if copylen < 134 {
        let nbits = log2_floor_non_zero((copylen - 6) as u64) - 1;
        (((nbits << 1) as usize) + ((copylen - 6) >> nbits) + 4) as u16
    } else if copylen < 2118 {
        log2_floor_non_zero((copylen - 70) as u64) as u16 + 12
    } else {
        23
    }
}

/// RFC 7932 §10.3 command symbol from (insert, copy, use_last).
fn combine_length_codes(inscode: u16, copycode: u16, use_last_distance: bool) -> u16 {
    let bits64 = (copycode & 0x7) | ((inscode & 0x7) << 3);
    if use_last_distance && inscode < 8 && copycode < 16 {
        if copycode < 8 {
            bits64
        } else {
            bits64 | 64
        }
    } else {
        let sub_offset = 2 * ((copycode >> 3) as i32 + 3 * (inscode >> 3) as i32);
        let offset = (sub_offset << 5) + 0x40 + (0x520d40i32 >> sub_offset & 0xc0);
        (offset as u16 as i32 | bits64 as i32) as u16
    }
}

#[inline]
fn insert_extra(code: usize) -> u32 {
    K_INS_EXTRA[code]
}

#[inline]
fn copy_extra(code: usize) -> u32 {
    K_COPY_EXTRA[code]
}

const K_INFINITY: f32 = 1.7e38;
const NUM_CMD_SYMBOLS: usize = 704;
/// BrotliCalculateDistanceCodeLimit(MAX_ALLOWED_DISTANCE, 3, 120).
const MAX_EFFECTIVE_DISTANCE_ALPHABET_SIZE: usize = 544;
const LONG_COPY_QUICK_STEP: usize = 16384;
/// MAX_ZOPFLI_LEN_QUALITY_10 / _11 (quality.h).
const MAX_ZOPFLI_LEN: [usize; 2] = [150, 325];

/// Cap on a single relaxed match length, and the reason it is BOUNDED
/// by default (issues #388 and #408): the per-position work that
/// scales with this cap — candidate length computation and sweep
/// stepping on repetitive content — makes a large ceiling a
/// content-dependent hang (a 65,536 cap was once claimed "measured
/// safe" and hung windows-latest CI for 23+ minutes on tens-of-KB
/// repetitive text; an uncapped 16.7M default was the same class).
/// 1,951 is the longest copy-length code before the 24-bit extended
/// forms. Larger values pay ratio on specific corpora (bin1 q11
/// −8.7KB, rustsrc q11 −3.7KB) but that is a documented trade, not a
/// default. Opt in per corpus with BROTLI_MLEN_CAP.
fn match_len_cap() -> usize {
    std::env::var("BROTLI_MLEN_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_951)
}
/// StartPosQueue depth (1.2.0: `q_[8]`; 1.1 had 5).
const SPQ_SIZE: usize = 8;
/// MaxZopfliCandidates: q10 = 1, q11 = 5.
const MAX_ZOPFLI_CANDIDATES: [usize; 2] = [1, 5];

const CACHE_INDEX: [usize; 16] = [0, 1, 2, 3, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1];
const CACHE_OFFSET: [i32; 16] = [0, 0, 0, 0, -1, 1, -2, 2, -3, 3, -1, 1, -2, 2, -3, 3];

#[inline]
fn fast_log2(v: usize) -> f32 {
    (v as f64 + 0.5).log2() as f32
}

/// One DP node per input position (upstream `ZopfliNode`). `length`
/// packs copy_len | (len_code modifier) << 25; `dcode_insert` packs
/// (short_code + 1) << 27 | insert_len.
#[derive(Clone, Copy)]
struct Node {
    length: u32,
    distance: u32,
    dcode_insert: u32,
    cost: f32,
    /// Index of the previous command-start position (the shortcut
    /// chain used to rebuild distance caches).
    shortcut: u32,
    /// Only used while tracing the shortest path.
    next: u32,
}

impl Node {
    #[inline]
    fn copy_len(&self) -> usize {
        (self.length & 0x1FF_FFFF) as usize
    }
    #[inline]
    fn len_code(&self) -> usize {
        self.copy_len() + 9 - (self.length >> 25) as usize
    }
    #[inline]
    fn insert_len(&self) -> usize {
        (self.dcode_insert & 0x07FF_FFFF) as usize
    }
    /// 0 = normal code, else the short code + 1.
    #[inline]
    fn short_code(&self) -> u32 {
        self.dcode_insert >> 27
    }
    #[inline]
    fn dist_code(&self) -> usize {
        let s = self.short_code();
        if s == 0 {
            self.distance as usize + 15
        } else {
            (s - 1) as usize
        }
    }
}

fn init_nodes(nodes: &mut [Node]) {
    for node in nodes.iter_mut() {
        *node = Node {
            length: 1,
            distance: 0,
            dcode_insert: 0,
            cost: K_INFINITY,
            shortcut: 0,
            next: 0,
        };
    }
}

/// One alternative command-start position (upstream `PosData`).
struct PosData {
    pos: usize,
    distance_cache: [i32; 4],
    costdiff: f32,
    cost: f32,
}

/// Upstream `StartPosQueue`: 8 most recent eligible starts, kept
/// sorted by costdiff (cost − literal-only cost).
struct StartPosQueue {
    q: Vec<PosData>,
    idx: usize,
}

impl StartPosQueue {
    fn new() -> Self {
        Self {
            q: (0..SPQ_SIZE)
                .map(|_| PosData {
                    pos: 0,
                    distance_cache: [0; 4],
                    costdiff: 0.0,
                    cost: K_INFINITY,
                })
                .collect(),
            idx: 0,
        }
    }

    fn size(&self) -> usize {
        self.idx.min(SPQ_SIZE)
    }

    fn push(&mut self, posdata: PosData) {
        let mut offset = !self.idx & 7;
        self.idx = self.idx.wrapping_add(1);
        let len = self.size();
        self.q[offset] = posdata;
        let mut i = 1;
        while i < len {
            if self.q[offset & 7].costdiff > self.q[(offset + 1) & 7].costdiff {
                self.q.swap(offset & 7, (offset + 1) & 7);
            }
            offset = offset.wrapping_add(1);
            i += 1;
        }
    }

    fn at(&self, k: usize) -> &PosData {
        &self.q[(k.wrapping_sub(self.idx)) & 7]
    }
}

/// Upstream `ZopfliCostModel`.
pub(crate) struct CostModel {
    pub(crate) cost_cmd: Vec<f32>,
    pub(crate) cost_dist: Vec<f32>,
    literal_costs: Vec<f32>,
    pub(crate) min_cost_cmd: f32,
}

/// Upstream `SetCost`: per-symbol Shannon cost with a fixed
/// missing-symbol penalty (+2 bits) for non-literal histograms.
pub(crate) fn set_cost(histogram: &[u32], literal_histogram: bool, cost: &mut [f32]) {
    let mut sum: u64 = 0;
    for &h in histogram.iter() {
        sum += u64::from(h);
    }
    let log2sum = fast_log2(sum as usize);
    let mut missing_symbol_sum = sum;
    if !literal_histogram {
        for &h in histogram.iter() {
            if h == 0 {
                missing_symbol_sum += 1;
            }
        }
    }
    let missing_symbol_cost = fast_log2(missing_symbol_sum as usize) + 2.0;
    for i in 0..histogram.len() {
        if histogram[i] == 0 {
            cost[i] = missing_symbol_cost;
            continue;
        }
        let mut c = log2sum - fast_log2(histogram[i] as usize);
        if c < 1.0 {
            c = 1.0;
        }
        cost[i] = c;
    }
}

impl CostModel {
    /// Pass-1 costs: sliding-window literal entropy (upstream
    /// `ZopfliCostModelSetFromLiteralCosts` +
    /// `BrotliEstimateBitCostsForLiterals`, non-UTF8 variant).
    pub(crate) fn from_literal_costs(data: &[u8]) -> Self {
        let n = data.len();
        let mut literal_costs = vec![0.0f32; n + 1];
        if is_mostly_utf8(data) {
            estimate_literal_bit_costs_utf8(data, &mut literal_costs[1..]);
        } else {
            estimate_literal_bit_costs(data, &mut literal_costs[1..]);
        }
        let mut carry = 0.0f32;
        literal_costs[0] = 0.0;
        for i in 0..n {
            carry += literal_costs[i + 1];
            literal_costs[i + 1] = literal_costs[i] + carry;
            carry -= literal_costs[i + 1] - literal_costs[i];
        }
        let mut cost_cmd = vec![0.0f32; NUM_CMD_SYMBOLS];
        for (i, c) in cost_cmd.iter_mut().enumerate() {
            *c = fast_log2(11 + i);
        }
        let mut cost_dist = vec![0.0f32; MAX_EFFECTIVE_DISTANCE_ALPHABET_SIZE];
        for (i, c) in cost_dist.iter_mut().enumerate() {
            *c = fast_log2(20 + i);
        }
        Self {
            cost_cmd,
            cost_dist,
            literal_costs,
            min_cost_cmd: fast_log2(11),
        }
    }

    /// Pass-2 costs: histograms over the previous pass's commands
    /// (upstream `ZopfliCostModelSetFromCommands`).
    pub(crate) fn from_commands(data: &[u8], commands: &[Command], mlen_offset: usize) -> Self {
        let n = data.len();
        let mut hist_literal = [0u32; 256];
        let mut hist_cmd = [0u32; NUM_CMD_SYMBOLS];
        let mut hist_dist = [0u32; MAX_EFFECTIVE_DISTANCE_ALPHABET_SIZE];
        let mut pos = 0usize;
        for cmd in commands {
            let ins = cmd.insert_len as usize;
            let copy = cmd.copy_len as usize;
            for j in 0..ins {
                if pos + j < n {
                    hist_literal[usize::from(data[pos + j])] += 1;
                }
            }
            pos += ins + copy;
            if pos > n {
                break;
            }
        }
        // Command/dist symbols: derive through the SAME rep-conversion
        // walk the emission uses (short codes 0-15 + implicit-rep0
        // cmd folding). Upstream SetFromCommands histograms each
        // command's stored dist_prefix_/cmd_prefix_, which encode rep
        // usage — that feedback is what makes rep codes cheap in the
        // next iteration's model. Deriving long-form symbols only (the
        // old approximation) left short codes unseen in the histogram,
        // so SetCost priced them as rare and the DP under-rode reps
        // (~3x fewer implicit-rep0 commands than the reference on CSV).
        let dist_cfg = crate::encoder::distance_config::DistanceConfig::choose(commands, 0);
        // The walk's dict classification is global-position based;
        // mlen_offset must be the chunk's real offset or every
        // dictionary candidate on continuation chunks mis-advances
        // (the arial q10 8 MiB-chunk overrun).
        if let Some(stream) =
            crate::from_spec_encoder::build_symbol_stream(commands, data, mlen_offset, &dist_cfg)
        {
            for &cs in &stream.cmd_symbols {
                if cs < NUM_CMD_SYMBOLS {
                    hist_cmd[cs] += 1;
                }
            }
            for &ds in &stream.dist_symbols {
                let s = ds as usize;
                if s < MAX_EFFECTIVE_DISTANCE_ALPHABET_SIZE {
                    hist_dist[s] += 1;
                }
            }
        }
        let mut cost_literal = [0.0f32; 256];
        set_cost(&hist_literal, true, &mut cost_literal);
        let mut cost_cmd = vec![0.0f32; NUM_CMD_SYMBOLS];
        set_cost(&hist_cmd, false, &mut cost_cmd);
        let mut cost_dist = vec![0.0f32; MAX_EFFECTIVE_DISTANCE_ALPHABET_SIZE];
        set_cost(&hist_dist, false, &mut cost_dist);
        let mut min_cost_cmd = K_INFINITY;
        for &c in cost_cmd.iter() {
            min_cost_cmd = min_cost_cmd.min(c);
        }
        let mut literal_costs = vec![0.0f32; n + 1];
        let mut carry = 0.0f32;
        for i in 0..n {
            carry += cost_literal[usize::from(data[i])];
            literal_costs[i + 1] = literal_costs[i] + carry;
            carry -= literal_costs[i + 1] - literal_costs[i];
        }
        Self {
            cost_cmd,
            cost_dist,
            literal_costs,
            min_cost_cmd,
        }
    }

    #[inline]
    fn cmd_cost(&self, cmdcode: u16) -> f32 {
        self.cost_cmd[usize::from(cmdcode)]
    }
    #[inline]
    fn dist_cost(&self, distcode: usize) -> f32 {
        self.cost_dist[distcode]
    }
    #[inline]
    pub(crate) fn lit_costs(&self, from: usize, to: usize) -> f32 {
        self.literal_costs[to] - self.literal_costs[from]
    }

    /// Replace the flat literal prefix-sum with per-position costs
    /// (e.g. context-partitioned SetCost tables). Used by the btopt
    /// parser to steer literals by (p1, p2) context.
    pub(crate) fn with_positional_literals(mut self, lit_pos: &[f32]) -> Self {
        let mut literal_costs = vec![0.0f32; lit_pos.len() + 1];
        let mut carry = 0.0f32;
        for (i, &c) in lit_pos.iter().enumerate() {
            carry += c;
            literal_costs[i + 1] = literal_costs[i] + carry;
            carry -= literal_costs[i + 1] - literal_costs[i];
        }
        self.literal_costs = literal_costs;
        self
    }
}

/// Upstream `UTF8Position`.
fn utf8_position(last: usize, c: usize, clamp: usize) -> usize {
    if c < 128 {
        0
    } else if c >= 192 {
        1.min(clamp)
    } else if last < 0xE0 {
        0
    } else {
        2.min(clamp)
    }
}

fn decide_multibyte_level(data: &[u8]) -> usize {
    let mut counts = [0usize; 3];
    let mut last_c = 0usize;
    for &b in data {
        let c = usize::from(b);
        counts[utf8_position(last_c, c, 2)] += 1;
        last_c = c;
    }
    let mut max_utf8 = 1;
    if counts[2] < 500 {
        max_utf8 = 1;
    }
    if counts[1] + counts[2] < 25 {
        max_utf8 = 0;
    }
    max_utf8
}

/// Upstream `BrotliIsMostlyUTF8` (kMinUTF8Ratio = 0.75): counts bytes
/// consumed by valid UTF-8 sequences.
fn is_mostly_utf8(data: &[u8]) -> bool {
    let mut size_utf8 = 0usize;
    let mut i = 0usize;
    let len = data.len();
    while i < len {
        let b = data[i];
        // BrotliParseAsUTF8: bytes consumed and symbol validity.
        let (bytes, valid) = if b < 0x80 {
            (1, true)
        } else if b < 0xC0 {
            (1, false)
        } else if b < 0xE0 {
            (2, i + 1 < len && (data[i + 1] & 0xC0) == 0x80)
        } else if b < 0xF0 {
            (
                3,
                i + 2 < len && (data[i + 1] & 0xC0) == 0x80 && (data[i + 2] & 0xC0) == 0x80,
            )
        } else if b < 0xF8 {
            (
                4,
                i + 3 < len
                    && (data[i + 1] & 0xC0) == 0x80
                    && (data[i + 2] & 0xC0) == 0x80
                    && (data[i + 3] & 0xC0) == 0x80,
            )
        } else {
            (1, false)
        };
        i += bytes;
        if valid {
            size_utf8 += bytes;
        }
    }
    (size_utf8 as f64) > 0.75 * (len as f64)
}

/// Upstream `EstimateBitCostsForLiteralsUTF8`: 495-byte sliding
/// window with 3 UTF-8 byte-position histograms and a 2000-byte
/// prologue surcharge.
fn estimate_literal_bit_costs_utf8(data: &[u8], cost: &mut [f32]) {
    let len = cost.len();
    let max_utf8 = decide_multibyte_level(data);
    let window_half = 495usize;
    let mut in_window = window_half.min(len);
    let mut in_window_utf8 = [0usize; 3];
    let mut histogram = vec![0u32; 3 * 256];
    {
        let mut last_c = 0usize;
        let mut utf8_pos = 0usize;
        for i in 0..in_window {
            let c = usize::from(data[i]);
            histogram[256 * utf8_pos + c] += 1;
            in_window_utf8[utf8_pos] += 1;
            utf8_pos = utf8_position(last_c, c, max_utf8);
            last_c = c;
        }
    }
    for i in 0..len {
        if i >= window_half {
            let c = if i < window_half + 1 {
                0
            } else {
                usize::from(data[i - window_half - 1])
            };
            let last_c = if i < window_half + 2 {
                0
            } else {
                usize::from(data[i - window_half - 2])
            };
            let p = utf8_position(last_c, c, max_utf8);
            histogram[256 * p + usize::from(data[i - window_half])] -= 1;
            in_window_utf8[p] -= 1;
        }
        if i + window_half < len {
            let c = usize::from(data[i + window_half - 1]);
            let last_c = usize::from(data[i + window_half - 2]);
            let p = utf8_position(last_c, c, max_utf8);
            histogram[256 * p + usize::from(data[i + window_half])] += 1;
            in_window_utf8[p] += 1;
        }
        let c = if i < 1 { 0 } else { usize::from(data[i - 1]) };
        let last_c = if i < 2 { 0 } else { usize::from(data[i - 2]) };
        let p = utf8_position(last_c, c, max_utf8);
        let mut histo = histogram[256 * p + usize::from(data[i])];
        if histo == 0 {
            histo = 1;
        }
        let mut lit_cost = fast_log2(in_window_utf8[p]) - fast_log2(histo as usize);
        lit_cost += 0.02905;
        if lit_cost < 1.0 {
            lit_cost *= 0.5;
            lit_cost += 0.5;
        }
        if i < 2000 {
            lit_cost += 0.35 + (0.35 / 2000.0) * i as f64 as f32;
        }
        cost[i] = lit_cost;
        let _ = &mut in_window;
        if i >= window_half {
            in_window -= 1;
        }
        if i + window_half < len {
            in_window += 1;
        }
    }
}

/// Upstream non-UTF8 `BrotliEstimateBitCostsForLiterals`: 2000-byte
/// sliding window per-byte entropy.
fn estimate_literal_bit_costs(data: &[u8], cost: &mut [f32]) {
    let len = cost.len();
    let window_half = 2000usize;
    let mut histogram = [0u32; 256];
    let mut in_window = window_half.min(len);
    for i in 0..in_window {
        histogram[usize::from(data[i])] += 1;
    }
    for i in 0..len {
        if i >= window_half {
            histogram[usize::from(data[i - window_half])] -= 1;
            in_window -= 1;
        }
        if i + window_half < len {
            histogram[usize::from(data[i + window_half])] += 1;
            in_window += 1;
        }
        let mut histo = histogram[usize::from(data[i])];
        if histo == 0 {
            histo = 1;
        }
        let mut lit_cost = fast_log2(in_window) - fast_log2(histo as usize);
        lit_cost += 0.029;
        if lit_cost < 1.0 {
            lit_cost *= 0.5;
            lit_cost += 0.5;
        }
        cost[i] = lit_cost;
    }
}

/// npostfix=0/ndirect=0 long-form distance symbol (upstream
/// PrefixEncodeCopyDistance low 10 bits).
pub(crate) fn long_dist_symbol(distance: u32) -> usize {
    if distance == 0 {
        return 0;
    }
    let d = distance - 1;
    let mut nbits = 1u32;
    while nbits < 24 {
        let limit = (3u64 << nbits) - 4;
        if u64::from(d) < limit {
            break;
        }
        nbits += 1;
    }
    let odd = u64::from(d) >= (3u64 << nbits) - 4;
    let distval = (nbits - 1) * 2 + odd as u32;
    16 + distval as usize
}

/// Collect per-position match lists once (upstream
/// `FindAllMatchesH10` semantics on the H10 binary tree, with the
/// quality-11 short-match scan at backward <= 64), applying the
/// long-copy skip: when the longest match exceeds MAX_ZOPFLI_LEN,
/// keep only it and zero the tail positions.
#[allow(clippy::type_complexity)]
pub(crate) fn collect_matches(
    data: &[u8],
    tree: &mut omnizip_codecs::BinaryTreeMatchFinder,
    quality: i32,
) -> (Vec<u32>, Vec<(u32, u32)>) {
    let n = data.len();
    let mut num_matches = vec![0u32; n];
    let mut matches: Vec<(u32, u32)> = Vec::new();
    let mut cur: Vec<omnizip_codecs::Lz77Match> = Vec::new();
    let short_max_backward: usize = if quality != 11 { 16 } else { 64 };
    let max_zopfli_len = MAX_ZOPFLI_LEN[if quality >= 11 { 1 } else { 0 }];
    let mut i = 0usize;
    while i + 4 < n {
        // Per-position base of the flat list, captured BEFORE the
        // short-match scan pushes its entries (they count too).
        let prior = matches.len();
        let mut best_len = 1usize;
        let mut stop = i.saturating_sub(short_max_backward);
        // Short-match scan: only while nothing longer than 2 found.
        let mut j = i;
        while j > stop && best_len <= 2 {
            let prev = j - 1;
            if data[i] != data[prev] || data[i + 1] != data[prev + 1] {
                j -= 1;
                continue;
            }
            let mut len = 0usize;
            while i + len < n && data[i + len] == data[prev + len] {
                len += 1;
            }
            if len > best_len {
                best_len = len;
                matches.push(((i - prev) as u32, len as u32));
            }
            j -= 1;
        }
        let _ = &mut stop;
        tree.store_and_find(i, &mut cur);
        if best_len < n - i {
            for m in cur.drain(..) {
                if (m.length as usize) > best_len {
                    best_len = m.length as usize;
                    matches.push((m.distance, m.length));
                }
            }
        }
        if best_len > max_zopfli_len {
            // Keep only the longest match, skip its tail positions
            // (they were stored into the tree above).
            let last = *matches.last().unwrap_or(&(0, 0));
            matches.truncate(prior);
            matches.push(last);
            num_matches[i] = 1;
            let skip = (last.1 as usize).saturating_sub(1).min(n - i - 1);
            let end = (i + 1 + skip).min(n);
            for k in (i + 1)..end {
                tree.store(k);
            }
            i = end;
        } else {
            num_matches[i] = (matches.len() - prior) as u32;
            i += 1;
        }
    }
    // matches is flat; per-position counts are num_matches — the
    // caller slices via prefix sums.
    (num_matches, matches)
}

/// Upstream `ComputeMinimumCopyLength`.
pub(crate) fn compute_minimum_copy_length<F>(
    start_cost: f32,
    cost_at: F,
    num_bytes: usize,
    pos: usize,
) -> usize
where
    F: Fn(usize) -> f32,
{
    let mut min_cost = start_cost;
    let mut len = 2usize;
    let mut next_len_bucket = 4usize;
    let mut next_len_offset = 10usize;
    while pos + len <= num_bytes && cost_at(pos + len) <= min_cost {
        len += 1;
        if len == next_len_offset {
            min_cost += 1.0;
            next_len_offset += next_len_bucket;
            next_len_bucket *= 2;
        }
    }
    len
}

/// Upstream `ComputeDistanceShortcut`: whether the command ENDING at
/// `pos` is a normal-distance command usable to rebuild caches.
/// Iterative — the C tail recursion overflows Rust stacks on command
/// chains thousands deep (all-zeros inputs).
fn compute_distance_shortcut(mut pos: usize, nodes: &[Node], gap: usize) -> u32 {
    while pos != 0 {
        let node = &nodes[pos];
        let c_len = node.copy_len();
        let i_len = node.insert_len();
        let dist = node.distance as usize;
        // Upstream: `ZopfliNodeDistanceCode(&nodes[pos]) > 0` — only
        // implicit-rep0 (code 0) fails to update the distance cache.
        // Untouched nodes (dist 0 → code 15) DO anchor; an extra
        // `dist > 0` guard made them walk back one position at a time,
        // turning long-repetitive input quadratic (task #312).
        if dist + c_len <= pos + gap && node.dist_code() > 0 {
            return pos as u32;
        }
        pos -= c_len + i_len;
    }
    0
}

/// Upstream `ComputeDistanceCache`: rebuild the 4-slot cache by
/// walking the shortcut chain back from `pos`.
fn compute_distance_cache(pos: usize, starting: &[i32; 4], nodes: &[Node], gap: usize) -> [i32; 4] {
    let mut cache = [0i32; 4];
    let mut idx = 0usize;
    let mut p = nodes[pos].shortcut as usize;
    while idx < 4 && p > 0 {
        let node = &nodes[p];
        cache[idx] = node.distance as i32;
        idx += 1;
        p = nodes[p - node.copy_len() - node.insert_len()].shortcut as usize;
    }
    while idx < 4 {
        cache[idx] = starting[idx];
        idx += 1;
    }
    cache
}

/// One DP step (upstream `UpdateNodes`): relax rep-code copies from
/// the queued alternative starts, then (first two starts only) the
/// H10 match list. Returns the longest relaxed copy length.
fn update_nodes(
    pos: usize,
    data: &[u8],
    nodes: &mut [Node],
    queue: &mut StartPosQueue,
    model: &CostModel,
    num_matches: u32,
    matches: &[(u32, u32)],
    starting_cache: &[i32; 4],
    max_candidates: usize,
    max_zopfli_len: usize,
) -> usize {
    let n = data.len();
    let max_len = n - pos;
    let mut result = 0usize;

    // EvaluateNode.
    let node_cost = nodes[pos].cost;
    nodes[pos].shortcut = compute_distance_shortcut(pos, nodes, 0);
    if node_cost <= model.lit_costs(0, pos) {
        queue.push(PosData {
            pos,
            distance_cache: compute_distance_cache(pos, starting_cache, nodes, 0),
            costdiff: node_cost - model.lit_costs(0, pos),
            cost: node_cost,
        });
    }

    let min_len = {
        let posdata = queue.at(0);
        let min_cost = posdata.cost + model.min_cost_cmd + model.lit_costs(posdata.pos, pos);
        compute_minimum_copy_length(min_cost, |p| nodes[p].cost, n, pos)
    };

    for k in 0..max_candidates.min(queue.size()) {
        let posdata = queue.at(k);
        let start = posdata.pos;
        let inscode = get_insert_length_code(pos - start);
        let start_costdiff = posdata.costdiff;
        let base_cost =
            start_costdiff + insert_extra(usize::from(inscode)) as f32 + model.lit_costs(0, pos);

        // Rep-code relaxation from this start's cache.
        let mut best_len = min_len.saturating_sub(1);
        let mut j = 0usize;
        while j < 16 && best_len < max_len {
            let idx = CACHE_INDEX[j];
            let backward = (posdata.distance_cache[idx] + CACHE_OFFSET[j]) as usize;
            let mut len = 0usize;
            if pos < backward || backward == 0 {
                j += 1;
                continue;
            }
            let prev = pos - backward;
            if pos + best_len < n && data[pos + best_len] != data[prev + best_len] {
                j += 1;
                continue;
            }
            let mut l = 0usize;
            while pos + l < n && data[pos + l] == data[prev + l] {
                l += 1;
            }
            crate::encoder::work_meter::add(0, l as u64);
            len = l;
            if len > best_len {
                let dist_cost = base_cost + model.dist_cost(j);
                let mut l2 = best_len + 1;
                while l2 <= len {
                    crate::encoder::work_meter::add(1, 1);
                    let copycode = get_copy_length_code(l2);
                    let cmdcode = combine_length_codes(inscode, copycode, j == 0);
                    let cost = if cmdcode < 128 { base_cost } else { dist_cost }
                        + copy_extra(usize::from(copycode)) as f32
                        + model.cmd_cost(cmdcode);
                    if cost < nodes[pos + l2].cost {
                        update_node(
                            nodes,
                            pos,
                            start,
                            l2,
                            l2,
                            backward as u32,
                            (j + 1) as u32,
                            cost,
                        );
                        result = result.max(l2);
                    }
                    best_len = l2;
                    l2 += 1;
                }
            }
            j += 1;
        }

        if k >= 2 {
            continue;
        }

        // H10 match list relaxation (normal distance codes). The cost
        // includes the distance extra bits (upstream distnumextra) —
        // long-form distances pay their 10-24 extra bits, without
        // which the DP over-copies massively.
        let mut len = min_len;
        for &(dist, mlen) in matches.iter().take(num_matches as usize) {
            let max_match_len = (mlen as usize).min(max_len).min(match_len_cap());
            let sym = long_dist_symbol(dist);
            let dist_extra_bits = ((sym as u32 - 16) >> 1) + 1;
            let dist_cost = base_cost + dist_extra_bits as f32 + model.dist_cost(sym);
            if len < max_match_len && max_match_len > max_zopfli_len {
                len = max_match_len;
            }
            while len <= max_match_len {
                crate::encoder::work_meter::add(2, 1);
                let copycode = get_copy_length_code(len);
                let cmdcode = combine_length_codes(inscode, copycode, false);
                let cost =
                    dist_cost + copy_extra(usize::from(copycode)) as f32 + model.cmd_cost(cmdcode);
                if cost < nodes[pos + len].cost {
                    update_node(nodes, pos, start, len, len, dist, 0, cost);
                    result = result.max(len);
                }
                len += 1;
            }
            // Whole-match jump: when the finder saw past the DP cap,
            // relax the FULL copy as a single command — the reference
            // takes monster matches whole (up to 122,894 B on bin1)
            // where the capped sweep re-pays the distance code every
            // 1,951 bytes. One extra relaxation per candidate: the
            // sweep length (the #388/#408 hang class) is untouched,
            // and copy code 23's 24-bit extra prices the jump exactly.
            let full_len = (mlen as usize).min(max_len);
            if full_len > max_match_len {
                crate::encoder::work_meter::add(2, 1);
                let copycode = get_copy_length_code(full_len);
                let cmdcode = combine_length_codes(inscode, copycode, false);
                let cost =
                    dist_cost + copy_extra(usize::from(copycode)) as f32 + model.cmd_cost(cmdcode);
                if cost < nodes[pos + full_len].cost {
                    update_node(nodes, pos, start, full_len, full_len, dist, 0, cost);
                    result = result.max(full_len);
                }
            }
        }
    }
    result
}

#[inline]
fn update_node(
    nodes: &mut [Node],
    pos: usize,
    start_pos: usize,
    len: usize,
    len_code: usize,
    dist: u32,
    short_code: u32,
    cost: f32,
) {
    let next = &mut nodes[pos + len];
    next.length = ((len as u32) & 0x1FF_FFFF) | ((((len + 9 - len_code) as u32) & 0x7F) << 25);
    next.distance = dist;
    next.dcode_insert = (short_code << 27) | ((pos - start_pos) as u32 & 0x07FF_FFFF);
    next.cost = cost;
}

/// Trace the shortest path back into commands.
fn shortest_path_commands(data: &[u8], nodes: &mut [Node]) -> Vec<Command> {
    let n = data.len();
    let mut index = n;
    while index > 0 && nodes[index].insert_len() == 0 && nodes[index].copy_len() == 1 {
        index -= 1;
    }
    nodes[index].next = u32::MAX;
    while index != 0 {
        let len = nodes[index].copy_len() + nodes[index].insert_len();
        index -= len;
        nodes[index].next = len as u32;
    }
    let mut commands = Vec::new();
    let mut pos = 0usize;
    let mut offset = nodes[0].next;
    while offset != u32::MAX {
        let next = &nodes[pos + offset as usize];
        let copy_len = next.copy_len();
        let insert_len = next.insert_len();
        let len_code = next.len_code();
        pos += insert_len;
        let distance = next.distance;
        let is_dict = len_code > copy_len;
        commands.push(Command {
            insert_len: insert_len as u32,
            // Our Command shape stores the WORD length for dict refs
            // (copy_len on the wire) and matchlen otherwise.
            copy_len: if is_dict {
                len_code as u32
            } else {
                copy_len as u32
            },
            distance,
        });
        pos += copy_len;
        offset = next.next;
    }
    if std::env::var("BROTLI_HQ_CMDDUMP").is_ok() {
        let mut pp = 0usize;
        eprintln!("HQ first 60 commands:");
        for c in &commands {
            eprintln!("HQ {pp} {} {} {}", c.insert_len, c.copy_len, c.distance);
            pp += c.insert_len as usize
                + if c.copy_len == 0 {
                    0
                } else {
                    c.copy_len as usize
                };
        }
    }
    // The reference folds an uncovered literal tail into
    // last_insert_len (consumed by the next block); our pipeline
    // expects an explicit trailing-insert command.
    if pos < data.len() {
        commands.push(Command {
            insert_len: (data.len() - pos) as u32,
            copy_len: 0,
            distance: 0,
        });
    }
    commands
}

/// Port of `BrotliCreateHqZopfliBackwardReferences` (quality 11):
/// collect matches once (with long-copy skip), then two DP passes —
/// pass 0 with sliding-window literal costs, pass 1 with histogram
/// costs from pass 0's commands.
pub fn parse_hq(input: &[u8], quality: i32) -> Vec<Command> {
    let n = input.len();
    if n < 8 {
        return Vec::new();
    }
    let mut tree = omnizip_codecs::BinaryTreeMatchFinder::new(input);
    let (num_matches, matches) = collect_matches(input, &mut tree, quality);
    parse_hq_with(input, quality, &num_matches, &matches)
}

/// Collection-sharing variant used by the q10/11 routing (the btopt
/// candidate consumes the same H10 list instead of re-walking the
/// tree). `num_matches`/`matches` come from [`collect_matches`].
pub(crate) fn parse_hq_with(
    input: &[u8],
    quality: i32,
    num_matches: &[u32],
    matches: &[(u32, u32)],
) -> Vec<Command> {
    let n = input.len();
    if n < 8 {
        return Vec::new();
    }
    let tier = if quality >= 11 { 1 } else { 0 };
    let max_zopfli_len = MAX_ZOPFLI_LEN[tier];
    let max_candidates = MAX_ZOPFLI_CANDIDATES[tier];
    // Upstream runs the cost-model refinement loop TWICE unconditionally
    // (backward_references_hq.c: `for (i = 0; i < 2; i++)`) — pass 2
    // derives costs from pass 1's commands, whose rep-code feedback is
    // worth ~17 ratio points on repetitive binary input (FITS q10 was
    // stuck at 1.28x reference size without it). BROTLI_HQ_1PASS
    // restores the old q10 single pass.
    let num_passes = if std::env::var("BROTLI_HQ_1PASS").is_ok() {
        1
    } else {
        2
    };
    // Prefix-sum offsets into the flat match list.
    let mut offsets = vec![0u32; n + 1];
    for i in 0..n {
        offsets[i + 1] = offsets[i] + num_matches[i];
    }

    let starting_cache: [i32; 4] = [16, 15, 11, 4];
    let mut nodes = vec![
        Node {
            length: 1,
            distance: 0,
            dcode_insert: 0,
            cost: 0.0,
            shortcut: 0,
            next: 0
        };
        n + 1
    ];
    let mut prev_commands: Vec<Command> = Vec::new();
    for pass in 0..num_passes {
        let model = if pass == 0 {
            CostModel::from_literal_costs(input)
        } else {
            CostModel::from_commands(input, &prev_commands, 0)
        };
        init_nodes(&mut nodes);
        nodes[0].length = 0;
        nodes[0].cost = 0.0;
        let mut queue = StartPosQueue::new();
        let mut i = 0usize;
        while i + 3 < n {
            let mstart = offsets[i] as usize;
            let mend = mstart + num_matches[i] as usize;
            if std::env::var("BROTLI_HQ_AT").is_ok_and(|v| v.parse::<usize>() == Ok(i)) {
                eprintln!("HQAT pos={i} matches={:?}", &matches[mstart..mend]);
            }
            let skip = update_nodes(
                i,
                input,
                &mut nodes,
                &mut queue,
                &model,
                num_matches[i],
                &matches[mstart..mend],
                &starting_cache,
                max_candidates,
                max_zopfli_len,
            );
            let mut skip = if skip < LONG_COPY_QUICK_STEP { 0 } else { skip };
            if num_matches[i] == 1 {
                let mlen = matches[mstart].1 as usize;
                if mlen > max_zopfli_len {
                    skip = skip.max(mlen);
                }
            }
            if skip > 1 {
                skip -= 1;
                while skip > 0 {
                    i += 1;
                    if i + 3 >= n {
                        break;
                    }
                    // EvaluateNode only (positions inside a long copy).
                    let node_cost = nodes[i].cost;
                    nodes[i].shortcut = compute_distance_shortcut(i, &nodes, 0);
                    if node_cost <= model.lit_costs(0, i) {
                        queue.push(PosData {
                            pos: i,
                            distance_cache: compute_distance_cache(i, &starting_cache, &nodes, 0),
                            costdiff: node_cost - model.lit_costs(0, i),
                            cost: node_cost,
                        });
                    }
                    skip -= 1;
                }
            }
            i += 1;
        }
        prev_commands = shortest_path_commands(input, &mut nodes);
    }
    prev_commands
}

#[cfg(test)]
mod mlen_cap_tests {
    use super::match_len_cap;

    /// The default match-length cap MUST stay bounded. Two shipped
    /// regressions came from raising it on fixture evidence alone:
    /// #388 (1951 → 65,536 hung windows-latest CI for 23+ minutes on
    /// tens-of-KB repetitive structured text) and #408 (an uncapped
    /// 16.7M default, same class). Per-position candidate and sweep
    /// work scales with this cap on repetitive content — "measured
    /// safe on my corpus" does not generalize. Bumping this requires
    /// a worst-case analysis on the pathological content class, not a
    /// benchmark win.
    #[test]
    fn match_len_cap_default_is_bounded() {
        // Guard against an inherited env var from a dev shell.
        std::env::remove_var("BROTLI_MLEN_CAP");
        assert_eq!(match_len_cap(), 1_951);
    }

    /// The #388/#408 content class: repetitive structured text
    /// (LimniFS issue-195-style log lines). Whole-file q11 encode of
    /// this shape hung CI when the cap was raised; this test exists
    /// so that class of change hangs HERE first.
    #[test]
    fn repetitive_structured_text_q11_completes_and_round_trips() {
        let payload = log_line_fixture(256);
        for q in [5, 11] {
            let out = crate::from_spec_encoder::compress_with_quality(&payload, q);
            let back = crate::decoder::decode(&out).unwrap_or_else(|e| panic!("q{q}: {e}"));
            assert_eq!(back, payload, "q{q} round trip");
        }
    }

    /// Deterministic hang guard: the #388/#408 incidents were WORK
    /// regressions (per-position candidate/sweep work multiplied on
    /// repetitive content), invisible to round-trip tests and only
    /// fatal on slow machines. The work meter counts every iteration
    /// of the knob-scaled DP loops; these budgets — calibrated at
    /// ~4x the 2026-08-30 measurements (zeros at 2x; see below) —
    /// fail a plain unit assertion the moment a change inflates DP
    /// work on the pathological classes, on any machine.
    ///
    /// Calibrations (2026-08-30, cap 1951):
    ///   loglines 16KB = 2.5M, 64KB = 11.9M, 256KB = 47.7M units
    ///   zeros 256KB = 12.57B units — the known #312 pathology,
    ///   budgeted at 2x to lock the floor while it remains open;
    ///   shrinking it (boundary-stepped rep sweeps) would allow a
    ///   much tighter budget.
    #[test]
    fn dp_work_budgets_on_pathological_content() {
        for (kb, budget) in [
            (16usize, 12_000_000u64),
            (64, 50_000_000),
            (256, 200_000_000),
        ] {
            let payload = log_line_fixture(kb);
            crate::encoder::work_meter::reset();
            let out = crate::from_spec_encoder::compress_with_quality(&payload, 11);
            let units = crate::encoder::work_meter::units();
            assert!(
                units > 0 && units < budget,
                "loglines {kb}KB q11: {units} work units >= budget {budget}                  (DP work inflated on repetitive content — see #388/#408)"
            );
            assert!(!out.is_empty());
        }

        // 32KB keeps the debug-mode suite fast: the #312 pathology's
        // per-position cost is size-independent (~20K units/position
        // post-budget; ~48K before), so any inflation is visible at
        // any fixture size.
        let zeros = vec![0u8; 32 * 1024];
        crate::encoder::work_meter::reset();
        let out = crate::from_spec_encoder::compress_with_quality(&zeros, 11);
        let units = crate::encoder::work_meter::units();
        assert!(
            units < 1_400_000_000,
            "zeros 32KB q11: {units} work units >= budget (the #312 pathology grew past \
             the btopt probe-budget ceiling — see the calibration note above)"
        );
    }

    fn log_line_fixture(kb: usize) -> Vec<u8> {
        let mut payload = String::with_capacity(kb * 1024);
        let mut line = 0usize;
        while payload.len() < kb * 1024 {
            use std::fmt::Write as _;
            let _ = writeln!(
                payload,
                "2026-08-26 service=api line={line} status={} bytes={}",
                200 + (line % 3) * 100,
                (line * 997) % 50_000
            );
            line += 1;
        }
        payload.into_bytes()
    }
}
