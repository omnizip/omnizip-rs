//! Reference port of brotli 1.2.0's block splitter and histogram
//! clustering (`block_splitter_inc.h`, `cluster_inc.h`,
//! `bit_cost_inc.h`, `block_splitter.c`): the FindBlocks DP over
//! sampled entropy codes, the batched HistogramPair priority-queue
//! clustering, and PopulationCost/BitsEntropy.
//!
//! This is the emission machinery behind the reference's literal
//! context trees (143 on FITS q11) and cmd/dist block splits.

const CODE_LENGTH_CODES: usize = 18;
const REPEAT_ZERO_CODE_LENGTH: usize = 17;
const HISTOGRAMS_PER_BATCH: usize = 64;
const CLUSTERS_PER_BATCH: usize = 16;
const MAX_NUMBER_OF_BLOCK_TYPES: usize = 256;

pub const MAX_LITERAL_HISTOGRAMS: usize = 100;
pub const MAX_COMMAND_HISTOGRAMS: usize = 50;
pub const LITERAL_BLOCK_SWITCH_COST: f64 = 28.1;
pub const COMMAND_BLOCK_SWITCH_COST: f64 = 13.5;
pub const DISTANCE_BLOCK_SWITCH_COST: f64 = 14.6;
pub const LITERAL_STRIDE_LENGTH: usize = 70;
pub const COMMAND_STRIDE_LENGTH: usize = 40;
pub const DISTANCE_STRIDE_LENGTH: usize = 40;
pub const SYMBOLS_PER_LITERAL_HISTOGRAM: usize = 544;
pub const SYMBOLS_PER_COMMAND_HISTOGRAM: usize = 530;
pub const SYMBOLS_PER_DISTANCE_HISTOGRAM: usize = 544;
pub const MIN_LENGTH_FOR_BLOCK_SPLITTING: usize = 128;

#[inline]
fn log2(v: u64) -> f64 {
    if v == 0 {
        return -2.0;
    }
    (v as f64).log2()
}

/// Upstream `BitCost`.
#[inline]
fn bit_cost(count: u64) -> f64 {
    if count == 0 {
        -2.0
    } else {
        log2(count)
    }
}

/// Upstream `BrotliBitsEntropy` over a histogram slice.
pub fn bits_entropy(population: &[u32]) -> f64 {
    let mut sum: u64 = 0;
    let mut retval = 0.0f64;
    for &p in population {
        let p = u64::from(p);
        sum += p;
        retval -= p as f64 * log2(p);
    }
    if sum > 0 {
        retval += sum as f64 * log2(sum);
    }
    if retval < sum as f64 {
        retval = sum as f64;
    }
    retval
}

/// Upstream `BrotliPopulationCost`: estimated bits to encode a
/// histogram's tree + payload, including the code-length-code model.
pub fn population_cost(data: &[u32]) -> f64 {
    const ONE: f64 = 12.0;
    const TWO: f64 = 20.0;
    const THREE: f64 = 28.0;
    const FOUR: f64 = 37.0;
    let data_size = data.len();
    let total: u64 = data.iter().map(|&x| u64::from(x)).sum();
    if total == 0 {
        return ONE;
    }
    let mut count = 0usize;
    let mut s = [0usize; 5];
    for i in 0..data_size {
        if data[i] > 0 {
            s[count] = i;
            count += 1;
            if count > 4 {
                break;
            }
        }
    }
    match count {
        1 => ONE,
        2 => TWO + total as f64,
        3 => {
            let h0 = u64::from(data[s[0]]);
            let h1 = u64::from(data[s[1]]);
            let h2 = u64::from(data[s[2]]);
            let hmax = h0.max(h1).max(h2);
            THREE + (2.0 * (h0 + h1 + h2) as f64) - hmax as f64
        }
        4 => {
            let mut histo = [
                u64::from(data[s[0]]),
                u64::from(data[s[1]]),
                u64::from(data[s[2]]),
                u64::from(data[s[3]]),
            ];
            histo.sort_unstable_by(|a, b| b.cmp(a));
            let h23 = histo[2] + histo[3];
            let hmax = h23.max(histo[0]);
            FOUR + 3.0 * h23 as f64 + 2.0 * (histo[0] + histo[1]) as f64 - hmax as f64
        }
        _ => {
            let mut max_depth = 1usize;
            let mut depth_histo = [0u32; CODE_LENGTH_CODES];
            let mut bits = 0.0f64;
            let log2total = log2(total);
            let mut i = 0usize;
            while i < data_size {
                if data[i] > 0 {
                    let log2p = log2total - log2(u64::from(data[i]));
                    let mut depth = (log2p + 0.5) as usize;
                    bits += u64::from(data[i]) as f64 * log2p;
                    if depth > 15 {
                        depth = 15;
                    }
                    if depth > max_depth {
                        max_depth = depth;
                    }
                    depth_histo[depth] += 1;
                    i += 1;
                } else {
                    let mut reps = 1usize;
                    let mut k = i + 1;
                    while k < data_size && data[k] == 0 {
                        reps += 1;
                        k += 1;
                    }
                    i += reps;
                    if i == data_size {
                        break;
                    }
                    if reps < 3 {
                        depth_histo[0] += reps as u32;
                    } else {
                        reps -= 2;
                        while reps > 0 {
                            depth_histo[REPEAT_ZERO_CODE_LENGTH] += 1;
                            bits += 3.0;
                            reps >>= 3;
                        }
                    }
                }
            }
            bits += (18 + 2 * max_depth) as f64;
            bits += bits_entropy(&depth_histo);
            bits
        }
    }
}

/// Variable-size histogram (alphabet 256 for literals, 704 commands,
/// ≤544+NPOSTFIX distances).
#[derive(Clone)]
pub struct Hist {
    pub data: Vec<u32>,
    pub total: u64,
    pub bit_cost: f64,
}

impl Hist {
    pub fn new(data_size: usize) -> Self {
        Self {
            data: vec![0; data_size],
            total: 0,
            bit_cost: 0.0,
        }
    }

    pub fn clear(&mut self) {
        self.data.iter_mut().for_each(|x| *x = 0);
        self.total = 0;
    }

    #[inline]
    pub fn add(&mut self, symbol: usize) {
        self.data[symbol] += 1;
        self.total += 1;
    }

    pub fn add_slice(&mut self, symbols: &[u16]) {
        for &s in symbols {
            self.add(usize::from(s));
        }
    }

    pub fn add_hist(&mut self, other: &Hist) {
        for (a, &b) in self.data.iter_mut().zip(other.data.iter()) {
            *a += b;
        }
        self.total += other.total;
    }

    pub fn recompute_cost(&mut self) {
        self.bit_cost = population_cost(&self.data);
    }
}

/// Upstream `HistogramPair`.
#[derive(Clone, Copy)]
struct HistogramPair {
    idx1: u32,
    idx2: u32,
    cost_diff: f64,
    cost_combo: f64,
}

#[inline]
fn pair_is_less(p1: &HistogramPair, p2: &HistogramPair) -> bool {
    if p1.cost_diff != p2.cost_diff {
        return p1.cost_diff > p2.cost_diff;
    }
    (p1.idx2 - p1.idx1) > (p2.idx2 - p2.idx1)
}

/// Upstream `ClusterCostDiff`: entropy reduction of the context map
/// when combining two clusters.
#[inline]
fn cluster_cost_diff(size_a: u64, size_b: u64) -> f64 {
    let size_c = size_a + size_b;
    size_a as f64 * log2(size_a) + size_b as f64 * log2(size_b) - size_c as f64 * log2(size_c)
}

/// Upstream `BrotliCompareAndPushToQueue`.
fn compare_and_push_to_queue(
    out: &[Hist],
    tmp: &mut Hist,
    cluster_size: &[u32],
    mut idx1: usize,
    mut idx2: usize,
    max_num_pairs: usize,
    pairs: &mut Vec<HistogramPair>,
) {
    if idx1 == idx2 {
        return;
    }
    if idx2 < idx1 {
        std::mem::swap(&mut idx1, &mut idx2);
    }
    let mut p = HistogramPair {
        idx1: idx1 as u32,
        idx2: idx2 as u32,
        cost_diff: 0.5
            * cluster_cost_diff(u64::from(cluster_size[idx1]), u64::from(cluster_size[idx2])),
        cost_combo: 0.0,
    };
    p.cost_diff -= out[idx1].bit_cost;
    p.cost_diff -= out[idx2].bit_cost;
    let is_good_pair;
    if out[idx1].total == 0 {
        p.cost_combo = out[idx2].bit_cost;
        is_good_pair = true;
    } else if out[idx2].total == 0 {
        p.cost_combo = out[idx1].bit_cost;
        is_good_pair = true;
    } else {
        let threshold = if pairs.is_empty() {
            1e99
        } else {
            pairs[0].cost_diff.max(0.0)
        };
        *tmp = out[idx1].clone();
        tmp.add_hist(&out[idx2]);
        let cost_combo = population_cost(&tmp.data);
        if cost_combo < threshold - p.cost_diff {
            p.cost_combo = cost_combo;
            is_good_pair = true;
        } else {
            is_good_pair = false;
        }
    }
    if is_good_pair {
        p.cost_diff += p.cost_combo;
        if !pairs.is_empty() && pair_is_less(&pairs[0], &p) {
            if pairs.len() < max_num_pairs {
                pairs.push(pairs[0]);
            }
            pairs[0] = p;
        } else if pairs.len() < max_num_pairs {
            pairs.push(p);
        }
    }
}

/// Upstream `BrotliHistogramCombine`. Returns the remaining number of
/// clusters; `out`, `cluster_size`, `symbols`, `clusters` are updated.
#[allow(clippy::too_many_arguments)]
fn histogram_combine(
    out: &mut [Hist],
    tmp: &mut Hist,
    cluster_size: &mut [u32],
    symbols: &mut [u32],
    clusters: &mut Vec<u32>,
    pairs: &mut Vec<HistogramPair>,
    mut num_clusters: usize,
    symbols_size: usize,
    max_clusters: usize,
    max_num_pairs: usize,
) -> usize {
    let mut cost_diff_threshold = 0.0f64;
    let mut min_cluster_size = 1usize;
    pairs.clear();

    for idx1 in 0..num_clusters {
        for idx2 in (idx1 + 1)..num_clusters {
            compare_and_push_to_queue(
                out,
                tmp,
                cluster_size,
                clusters[idx1] as usize,
                clusters[idx2] as usize,
                max_num_pairs,
                pairs,
            );
        }
    }

    while num_clusters > min_cluster_size {
        if pairs[0].cost_diff >= cost_diff_threshold {
            cost_diff_threshold = 1e99;
            min_cluster_size = max_clusters;
            continue;
        }
        let best_idx1 = pairs[0].idx1 as usize;
        let best_idx2 = pairs[0].idx2 as usize;
        out[best_idx1].add_hist(&out[best_idx2].clone());
        out[best_idx1].bit_cost = pairs[0].cost_combo;
        cluster_size[best_idx1] += cluster_size[best_idx2];
        for s in symbols.iter_mut().take(symbols_size) {
            if *s == best_idx2 as u32 {
                *s = best_idx1 as u32;
            }
        }
        clusters.retain(|&c| c != best_idx2 as u32);
        num_clusters -= 1;
        {
            let mut copy_to = 0usize;
            let mut i = 0usize;
            while i < pairs.len() {
                let p = pairs[i];
                if p.idx1 as usize == best_idx1
                    || p.idx2 as usize == best_idx1
                    || p.idx1 as usize == best_idx2
                    || p.idx2 as usize == best_idx2
                {
                    i += 1;
                    continue;
                }
                if pair_is_less(&pairs[0], &p) {
                    let front = pairs[0];
                    pairs[0] = p;
                    pairs[copy_to] = front;
                } else {
                    pairs[copy_to] = p;
                }
                copy_to += 1;
                i += 1;
            }
            pairs.truncate(copy_to);
        }
        for i in 0..num_clusters {
            compare_and_push_to_queue(
                out,
                tmp,
                cluster_size,
                best_idx1,
                clusters[i] as usize,
                max_num_pairs,
                pairs,
            );
        }
    }
    num_clusters
}

/// Upstream `BrotliHistogramBitCostDistance`.
fn bit_cost_distance(histogram: &Hist, candidate: &Hist, tmp: &mut Hist) -> f64 {
    if histogram.total == 0 {
        return 0.0;
    }
    *tmp = histogram.clone();
    tmp.add_hist(candidate);
    population_cost(&tmp.data) - candidate.bit_cost
}

/// Upstream `BrotliClusterHistograms`: cluster `input` histograms into
/// at most `max_histograms`; returns (clustered histograms, symbol →
/// cluster map).
pub fn cluster_histograms(input: &[Hist], max_histograms: usize) -> (Vec<Hist>, Vec<u32>) {
    let in_size = input.len();
    let data_size = input.first().map_or(256, |h| h.data.len());
    let mut out: Vec<Hist> = input.to_vec();
    let mut histogram_symbols: Vec<u32> = Vec::with_capacity(in_size);
    let mut cluster_size = vec![1u32; in_size];
    let mut clusters: Vec<u32> = Vec::with_capacity(in_size);
    let mut tmp = Hist::new(data_size);
    let max_input_histograms = 64usize;
    let pairs_capacity = max_input_histograms * max_input_histograms / 2;
    let mut pairs: Vec<HistogramPair> = Vec::with_capacity(pairs_capacity + 1);

    for h in out.iter_mut() {
        h.recompute_cost();
    }
    for i in 0..in_size {
        histogram_symbols.push(i as u32);
    }

    let mut num_clusters = 0usize;
    let mut i = 0usize;
    while i < in_size {
        let num_to_combine = (in_size - i).min(max_input_histograms);
        for j in 0..num_to_combine {
            clusters.push((i + j) as u32);
        }
        let n = histogram_combine(
            &mut out,
            &mut tmp,
            &mut cluster_size,
            &mut histogram_symbols[i..],
            &mut clusters,
            &mut pairs,
            num_to_combine,
            num_to_combine,
            max_histograms,
            pairs_capacity,
        );
        num_clusters += n;
        i += num_to_combine;
    }

    let max_num_pairs = (64 * num_clusters).min((num_clusters / 2) * num_clusters);
    num_clusters = histogram_combine(
        &mut out,
        &mut tmp,
        &mut cluster_size,
        &mut histogram_symbols,
        &mut clusters,
        &mut pairs,
        num_clusters,
        in_size,
        max_histograms,
        max_num_pairs,
    );

    // Remap: best out-histogram per input histogram, then recompute.
    for i in 0..in_size {
        let mut best_out = if i == 0 {
            histogram_symbols[0]
        } else {
            histogram_symbols[i - 1]
        } as usize;
        let mut best_bits = bit_cost_distance(&input[i], &out[best_out], &mut tmp);
        for &c in clusters.iter().take(num_clusters) {
            let cur = bit_cost_distance(&input[i], &out[c as usize], &mut tmp);
            if cur < best_bits {
                best_bits = cur;
                best_out = c as usize;
            }
        }
        histogram_symbols[i] = best_out as u32;
    }
    for &c in clusters.iter().take(num_clusters) {
        out[c as usize].clear();
    }
    for i in 0..in_size {
        let dst = histogram_symbols[i] as usize;
        out[dst].add_hist(&input[i]);
    }

    // Reindex: compact first-seen order.
    let invalid = u32::MAX;
    let mut new_index = vec![invalid; in_size];
    let mut next_index = 0u32;
    for &s in &histogram_symbols {
        if new_index[s as usize] == invalid {
            new_index[s as usize] = next_index;
            next_index += 1;
        }
    }
    let mut compact: Vec<Hist> = Vec::with_capacity(next_index as usize);
    let mut seen = vec![false; in_size];
    for &s in &histogram_symbols {
        let s = s as usize;
        if !seen[s] {
            seen[s] = true;
            compact.push(out[s].clone());
        }
    }
    for s in histogram_symbols.iter_mut() {
        *s = new_index[*s as usize];
    }
    (compact, histogram_symbols)
}

#[inline]
fn my_rand(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(16807);
    *seed
}

/// Upstream `BlockSplit`.
#[derive(Clone)]
pub struct BlockSplit {
    pub num_types: usize,
    pub num_blocks: usize,
    pub types: Vec<u8>,
    pub lengths: Vec<u32>,
}

impl BlockSplit {
    fn trivial(length: usize) -> Self {
        Self {
            num_types: 1,
            num_blocks: 1,
            types: vec![0],
            lengths: vec![length as u32],
        }
    }
}

/// Upstream `InitialEntropyCodes`.
fn initial_entropy_codes(data: &[u16], num_histograms: usize, stride: usize, hists: &mut [Hist]) {
    let length = data.len();
    let mut seed = 7u32;
    let block_length = length / num_histograms;
    for h in hists.iter_mut() {
        h.clear();
    }
    for i in 0..num_histograms {
        let mut pos = length * i / num_histograms;
        if i != 0 {
            pos += (my_rand(&mut seed) as usize) % block_length;
        }
        if pos + stride >= length {
            pos = length - stride - 1;
        }
        hists[i].add_slice(&data[pos..pos + stride]);
    }
}

/// Upstream `RandomSample`.
fn random_sample(seed: &mut u32, data: &[u16], mut stride: usize, sample: &mut Hist) {
    let length = data.len();
    let mut pos = 0usize;
    if stride >= length {
        stride = length;
    } else {
        pos = (my_rand(seed) as usize) % (length - stride + 1);
    }
    sample.add_slice(&data[pos..pos + stride]);
}

/// Upstream `RefineEntropyCodes`.
fn refine_entropy_codes(data: &[u16], stride: usize, num_histograms: usize, hists: &mut [Hist]) {
    let length = data.len();
    let iters = 2 * length / stride + 100;
    let mut seed = 7u32;
    let tmp = &mut Hist::new(hists[0].data.len());
    let iters = ((iters + num_histograms - 1) / num_histograms) * num_histograms;
    for iter in 0..iters {
        tmp.clear();
        random_sample(&mut seed, data, stride, tmp);
        hists[iter % num_histograms].add_hist(tmp);
    }
}

/// Upstream `FindBlocks`: DP over the symbol stream assigning block
/// ids; returns the number of blocks.
#[allow(clippy::too_many_arguments)]
fn find_blocks(
    data: &[u16],
    block_switch_bitcost: f64,
    num_histograms: usize,
    histograms: &[Hist],
    insert_cost: &mut [f64],
    cost: &mut [f64],
    switch_signal: &mut [u8],
    block_id: &mut [u8],
) -> usize {
    let length = data.len();
    let alphabet_size = histograms[0].data.len();
    let bitmap_len = (num_histograms + 7) >> 3;
    let mut num_blocks = 1usize;

    if num_histograms <= 1 {
        block_id.iter_mut().for_each(|b| *b = 0);
        return 1;
    }

    insert_cost.iter_mut().for_each(|c| *c = 0.0);
    for (i, h) in histograms.iter().enumerate() {
        insert_cost[i] = log2(h.total);
    }
    let mut i = alphabet_size;
    while i != 0 {
        i -= 1;
        for j in 0..num_histograms {
            insert_cost[i * num_histograms + j] =
                insert_cost[j] - bit_cost(u64::from(histograms[j].data[i]));
        }
    }

    cost.iter_mut().for_each(|c| *c = 0.0);
    switch_signal.iter_mut().for_each(|s| *s = 0);
    for byte_ix in 0..length {
        let ix = byte_ix * bitmap_len;
        let symbol = usize::from(data[byte_ix]);
        let insert_cost_ix = symbol * num_histograms;
        let mut min_cost = 1e99f64;
        let mut block_switch_cost = block_switch_bitcost;
        for k in 0..num_histograms {
            cost[k] += insert_cost[insert_cost_ix + k];
            if cost[k] < min_cost {
                min_cost = cost[k];
                block_id[byte_ix] = k as u8;
            }
        }
        if byte_ix < 2000 {
            block_switch_cost *= 0.77 + (0.07 / 2000.0) * byte_ix as f64;
        }
        for k in 0..num_histograms {
            cost[k] -= min_cost;
            if cost[k] >= block_switch_cost {
                let mask = 1u8 << (k & 7);
                cost[k] = block_switch_cost;
                switch_signal[ix + (k >> 3)] |= mask;
            }
        }
    }

    let mut byte_ix = length - 1;
    let mut ix = byte_ix * bitmap_len;
    let mut cur_id = block_id[byte_ix];
    while byte_ix > 0 {
        let mask = 1u8 << (cur_id & 7);
        byte_ix -= 1;
        ix -= bitmap_len;
        if switch_signal[ix + usize::from(cur_id >> 3)] & mask != 0 && cur_id != block_id[byte_ix] {
            cur_id = block_id[byte_ix];
            num_blocks += 1;
        }
        block_id[byte_ix] = cur_id;
    }
    num_blocks
}

fn remap_block_ids(block_ids: &mut [u8], num_histograms: usize) -> usize {
    let invalid = 256u16;
    let mut new_id = vec![invalid; num_histograms];
    let mut next_id = 0u16;
    for &b in block_ids.iter() {
        if new_id[usize::from(b)] == invalid {
            new_id[usize::from(b)] = next_id;
            next_id += 1;
        }
    }
    for b in block_ids.iter_mut() {
        *b = new_id[usize::from(*b)] as u8;
    }
    next_id as usize
}

fn build_block_histograms(data: &[u16], block_ids: &[u8], hists: &mut [Hist]) {
    for h in hists.iter_mut() {
        h.clear();
    }
    for (d, &b) in data.iter().zip(block_ids.iter()) {
        hists[usize::from(b)].add(usize::from(*d));
    }
}

/// Upstream `ClusterBlocks`: batched pre-clustering + final clustering,
/// then per-block best-histogram assignment.
fn cluster_blocks(data: &[u16], num_blocks: usize, block_ids: &[u8], split: &mut BlockSplit) {
    let data_size = data.first().map_or(256, |_| 256);
    let mut histogram_symbols = vec![0u32; num_blocks];
    let expected_num_clusters =
        CLUSTERS_PER_BATCH * (num_blocks + HISTOGRAMS_PER_BATCH - 1) / HISTOGRAMS_PER_BATCH;
    let mut all_histograms: Vec<Hist> = Vec::with_capacity(expected_num_clusters);
    let mut cluster_size: Vec<u32> = Vec::with_capacity(expected_num_clusters);
    let mut num_clusters = 0usize;

    // Block lengths from repeating ids.
    let mut block_lengths = vec![0u32; num_blocks];
    {
        let mut block_idx = 0usize;
        for i in 0..data.len() {
            block_lengths[block_idx] += 1;
            if i + 1 == data.len() || block_ids[i] != block_ids[i + 1] {
                block_idx += 1;
            }
        }
    }

    let batch = num_blocks.min(HISTOGRAMS_PER_BATCH);
    let mut histograms: Vec<Hist> = (0..batch).map(|_| Hist::new(data_size)).collect();
    let mut max_num_pairs = HISTOGRAMS_PER_BATCH * HISTOGRAMS_PER_BATCH / 2;
    let mut pairs: Vec<HistogramPair> = Vec::with_capacity(max_num_pairs + 1);
    let mut tmp = Hist::new(data_size);
    let mut pos = 0usize;

    let mut i = 0usize;
    while i < num_blocks {
        let num_to_combine = (num_blocks - i).min(HISTOGRAMS_PER_BATCH);
        let mut sizes = vec![1u32; num_to_combine];
        let mut symbols: Vec<u32> = vec![0u32; num_to_combine];
        let mut new_clusters: Vec<u32> = vec![0u32; num_to_combine];
        for j in 0..num_to_combine {
            histograms[j].clear();
            for _ in 0..block_lengths[i + j] {
                histograms[j].add(usize::from(data[pos]));
                pos += 1;
            }
            histograms[j].recompute_cost();
            new_clusters[j] = j as u32;
            symbols[j] = j as u32;
        }
        let num_new_clusters = histogram_combine(
            &mut histograms,
            &mut tmp,
            &mut sizes,
            &mut symbols,
            &mut new_clusters,
            &mut pairs,
            num_to_combine,
            num_to_combine,
            HISTOGRAMS_PER_BATCH,
            max_num_pairs,
        );
        let mut remap = vec![0u32; num_to_combine];
        for j in 0..num_new_clusters {
            let c = new_clusters[j] as usize;
            all_histograms.push(histograms[c].clone());
            cluster_size.push(sizes[c]);
            remap[c] = j as u32;
        }
        for j in 0..num_to_combine {
            histogram_symbols[i + j] = num_clusters as u32 + remap[symbols[j] as usize];
        }
        num_clusters += num_new_clusters;
        i += HISTOGRAMS_PER_BATCH;
    }

    max_num_pairs = (64 * num_clusters).min((num_clusters / 2) * num_clusters);
    pairs.clear();
    let mut clusters: Vec<u32> = (0..num_clusters as u32).collect();
    let num_final_clusters = histogram_combine(
        &mut all_histograms,
        &mut tmp,
        &mut cluster_size,
        &mut histogram_symbols,
        &mut clusters,
        &mut pairs,
        num_clusters,
        num_blocks,
        MAX_NUMBER_OF_BLOCK_TYPES,
        max_num_pairs,
    );

    // Assign each block to its best final histogram.
    let invalid_index = u32::MAX;
    let mut new_index = vec![invalid_index; num_clusters];
    pos = 0;
    let mut next_index = 0u32;
    for blk in 0..num_blocks {
        tmp.clear();
        for _ in 0..block_lengths[blk] {
            tmp.add(usize::from(data[pos]));
            pos += 1;
        }
        let mut best_out = if blk == 0 {
            histogram_symbols[0]
        } else {
            histogram_symbols[blk - 1]
        } as usize;
        let mut best_bits =
            bit_cost_distance(&tmp, &all_histograms[best_out], &mut Hist::new(data_size));
        for &c in clusters.iter().take(num_final_clusters) {
            let cur =
                bit_cost_distance(&tmp, &all_histograms[c as usize], &mut Hist::new(data_size));
            if cur < best_bits {
                best_bits = cur;
                best_out = c as usize;
            }
        }
        histogram_symbols[blk] = best_out as u32;
        if new_index[best_out] == invalid_index {
            new_index[best_out] = next_index;
            next_index += 1;
        }
    }

    // Rewrite as a block split.
    split.types.clear();
    split.lengths.clear();
    let mut cur_length = 0u32;
    let mut max_type = 0u8;
    for i in 0..num_blocks {
        cur_length += block_lengths[i];
        if i + 1 == num_blocks || histogram_symbols[i] != histogram_symbols[i + 1] {
            let id = new_index[histogram_symbols[i] as usize] as u8;
            split.types.push(id);
            split.lengths.push(cur_length);
            max_type = max_type.max(id);
            cur_length = 0;
        }
    }
    split.num_blocks = split.types.len();
    split.num_types = usize::from(max_type) + 1;
}

/// Upstream `SplitByteVector` (literal flavor: u16-carried symbols).
pub fn split_byte_vector(
    data: &[u16],
    symbols_per_histogram: usize,
    max_histograms: usize,
    sampling_stride_length: usize,
    block_switch_cost: f64,
    iters: usize,
) -> BlockSplit {
    let length = data.len();
    if length == 0 {
        return BlockSplit {
            num_types: 1,
            num_blocks: 0,
            types: Vec::new(),
            lengths: Vec::new(),
        };
    }
    if length < MIN_LENGTH_FOR_BLOCK_SPLITTING {
        return BlockSplit::trivial(length);
    }
    let mut num_histograms = length / symbols_per_histogram + 1;
    if num_histograms > max_histograms {
        num_histograms = max_histograms;
    }

    let mut histograms: Vec<Hist> = (0..num_histograms + 1).map(|_| Hist::new(256)).collect();
    let (hists, _tmp) = histograms.split_at_mut(num_histograms);
    initial_entropy_codes(data, num_histograms, sampling_stride_length, hists);
    refine_entropy_codes(data, sampling_stride_length, num_histograms, hists);

    let mut block_ids = vec![0u8; length];
    let mut num_blocks = 0usize;
    let alphabet_size = 256usize;
    let max_histograms_buf = num_histograms;
    let mut insert_cost = vec![0.0f64; alphabet_size * max_histograms_buf];
    let mut cost = vec![0.0f64; num_histograms];
    let bitmaplen = (num_histograms + 7) >> 3;
    let mut switch_signal = vec![0u8; length * bitmaplen];
    for _ in 0..iters {
        num_blocks = find_blocks(
            data,
            block_switch_cost,
            num_histograms,
            hists,
            &mut insert_cost,
            &mut cost,
            &mut switch_signal,
            &mut block_ids,
        );
        num_histograms = remap_block_ids(&mut block_ids, num_histograms);
        build_block_histograms(data, &block_ids, &mut hists[..num_histograms]);
    }

    let mut split = BlockSplit {
        num_types: 1,
        num_blocks: 0,
        types: Vec::new(),
        lengths: Vec::new(),
    };
    cluster_blocks(data, num_blocks, &block_ids, &mut split);
    split
}
