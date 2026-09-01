//! Optimal (btopt/btultra/btultra2) parser — port of
//! `~/src/external/zstd/lib/compress/zstd_opt.c`.
//!
//! Price-parse DP over "stretches" (a match followed by literals),
//! with adaptive frequency statistics (`ZSTD_rescaleFreqs` /
//! `ZSTD_updateStats`) and the binary-tree all-matches collector
//! (`ZSTD_insertBtAndGetAllMatches`). Sequences are stored with real
//! byte offsets into the `SeqStore`; the sequence encoder performs
//! repcode detection downstream, so this parser's internal offBase
//! model (1..3 = repcode, `offset + 3` = real offset) is resolved to
//! real offsets at store time.

use super::match_finder::{rotate_reps, RawSequence, SeqStore};
use super::sequences::{ll_code, ml_code, LL_BITS, ML_BITS};

const OPT_NUM: usize = 1 << 12;
const OPT_SIZE: usize = OPT_NUM + 3;
const MAX_PRICE: i32 = 1 << 30;
const BITCOST_MULTIPLIER: i32 = 1 << 8;
const LITFREQ_ADD: u32 = 2;
const PREDEF_THRESHOLD: usize = 8;
const REP_NUM: usize = 3;
const HASHLOG3_MAX: u32 = 17;

/// Seed stats for the first block (C `baseLLfreqs`).
const BASE_LL_FREQS: [u32; 36] = [
    4, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1,
];
/// Seed stats for the first block (C `baseOFCfreqs`).
const BASE_OF_FREQS: [u32; 32] = [
    6, 2, 1, 1, 2, 3, 4, 4, 4, 3, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
];

fn highbit32(v: u32) -> u32 {
    31 - v.leading_zeros()
}

/// Estimated cost of a frequency stat in whole bits (`optLevel == 0`).
fn bit_weight(stat: u32) -> i32 {
    (highbit32(stat + 1) as i32) << 8
}

/// Fractional-bit cost via linear interpolation (`optLevel != 0`).
fn frac_weight(raw_stat: u32) -> i32 {
    let stat = raw_stat + 1;
    let hb = highbit32(stat);
    let b_weight = (hb as i32) << 8;
    // (stat << 8) >> hb, computed in u64 to avoid overflow.
    let f_weight = (((stat as u64) << 8) >> hb) as i32;
    b_weight + f_weight
}

fn weight(stat: u32, opt_level: i32) -> i32 {
    if opt_level != 0 {
        frac_weight(stat)
    } else {
        bit_weight(stat)
    }
}

macro_rules! prices_of {
    ($st:expr) => {
        Prices {
            lit_freq: &$st.lit_freq,
            lit_sum_base_price: $st.lit_sum_base_price,
            lit_length_freq: &$st.lit_length_freq,
            lit_length_sum_base_price: $st.lit_length_sum_base_price,
            match_length_freq: &$st.match_length_freq,
            match_length_sum_base_price: $st.match_length_sum_base_price,
            off_code_freq: &$st.off_code_freq,
            off_code_sum_base_price: $st.off_code_sum_base_price,
            price_type_predef: $st.price_type_predef,
            opt_level: $st.opt_level,
        }
    };
}

/// Immutable snapshot of the price model, so the DP loops can hold
/// `&mut price_table` + `&match_table` at the same time.
struct Prices<'a> {
    lit_freq: &'a [u32; 256],
    lit_sum_base_price: i32,
    lit_length_freq: &'a [u32; 36],
    lit_length_sum_base_price: i32,
    match_length_freq: &'a [u32; 53],
    match_length_sum_base_price: i32,
    off_code_freq: &'a [u32; 32],
    off_code_sum_base_price: i32,
    price_type_predef: bool,
    opt_level: i32,
}

impl Prices<'_> {
    fn raw_literals_cost(&self, literals: &[u8]) -> i32 {
        let lit_length = literals.len();
        if lit_length == 0 {
            return 0;
        }
        if self.price_type_predef {
            return (lit_length as i32 * 6) << 8;
        }
        let mut price = self.lit_sum_base_price * lit_length as i32;
        let lit_price_max = self.lit_sum_base_price - BITCOST_MULTIPLIER;
        for &lit in literals {
            let mut lit_price = weight(self.lit_freq[lit as usize], self.opt_level);
            if lit_price > lit_price_max {
                lit_price = lit_price_max;
            }
            price -= lit_price;
        }
        price
    }

    fn lit_length_price(&self, lit_length: u32) -> i32 {
        if self.price_type_predef {
            return weight(lit_length, self.opt_level);
        }
        const BLOCKSIZE_MAX: u32 = 1 << 17;
        if lit_length == BLOCKSIZE_MAX {
            return BITCOST_MULTIPLIER + self.lit_length_price(BLOCKSIZE_MAX - 1);
        }
        let ll_code_idx = ll_code_fast(lit_length);
        ((LL_BITS[ll_code_idx as usize] as i32) << 8) + self.lit_length_sum_base_price
            - weight(self.lit_length_freq[ll_code_idx as usize], self.opt_level)
    }

    fn get_match_price(&self, off_base: u32, match_length: u32) -> i32 {
        let off_code = highbit32(off_base);
        if self.price_type_predef {
            let ml_base = match_length - 3;
            return weight(ml_base, self.opt_level) + ((16 + off_code as i32) << 8);
        }
        let mut price = ((off_code as i32) << 8) + self.off_code_sum_base_price
            - weight(self.off_code_freq[off_code as usize], self.opt_level);
        if self.opt_level < 2 && off_code >= 20 {
            price += ((off_code as i32 - 19) * 2) << 8;
        }
        let ml_code_idx = ml_code_fast(match_length);
        price += ((ML_BITS[ml_code_idx as usize] as i32) << 8) + self.match_length_sum_base_price
            - weight(self.match_length_freq[ml_code_idx as usize], self.opt_level);
        price + (BITCOST_MULTIPLIER / 5)
    }
}

/// O(1) LL code (C `ZSTD_LLcode`); the sequences.rs `ll_code` scans
/// LL_BASE linearly, which dominates when called per DP position.
fn ll_code_fast(lit_length: u32) -> u32 {
    const LL_CODE: [u32; 64] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 16, 17, 17, 18, 18, 19, 19, 20,
        20, 20, 20, 21, 21, 21, 21, 22, 22, 22, 22, 22, 22, 22, 22, 23, 23, 23, 23, 23, 23, 23, 23,
        24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    ];
    if lit_length > 63 {
        highbit32(lit_length) + 19
    } else {
        LL_CODE[lit_length as usize]
    }
}

/// O(1) ML code from the FULL match length (C `ZSTD_MLcode` on
/// `mlBase = ml - 3`).
fn ml_code_fast(match_length: u32) -> u32 {
    const ML_CODE: [u32; 128] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 30, 31, 32, 32, 33, 33, 34, 34, 35, 35, 36, 36, 36, 36, 37, 37, 37, 37,
        38, 38, 38, 38, 38, 38, 38, 38, 39, 39, 39, 39, 39, 39, 39, 39, 40, 40, 40, 40, 40, 40, 40,
        40, 40, 40, 40, 40, 40, 40, 40, 40, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41,
        41, 41, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42,
        42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42,
    ];
    let ml_base = match_length - 3;
    if ml_base > 127 {
        highbit32(ml_base) + 36
    } else {
        ML_CODE[ml_base as usize]
    }
}

/// offBase numeric representation: 1..3 = repcode, else offset + 3.
fn offset_to_offbase(o: u32) -> u32 {
    debug_assert!(o > 0);
    o + REP_NUM as u32
}

/// `ZSTD_updateRep` on a real-offset offBase path; also used to keep
/// the parser's internal offset history in sync with emission.
fn update_rep_by_offset(rep: &mut [u32; 3], offset: u32) {
    rep[2] = rep[1];
    rep[1] = rep[0];
    rep[0] = offset;
}

/// One entry of the price table: a "stretch" (match followed by
/// literals) in the C's terms.
#[derive(Clone, Copy, Default)]
struct Optimal {
    price: i32,
    off: u32,
    mlen: u32,
    litlen: u32,
    rep: [u32; 3],
}

#[derive(Clone, Copy, Default)]
struct OptMatch {
    off: u32,
    len: u32,
}

/// Price statistics + binary-tree match-finder state for the opt
/// parser. One per frame.
pub struct OptState {
    // Frequency tables (u32 like the C optState_t).
    lit_freq: [u32; 256],
    lit_sum: u32,
    lit_sum_base_price: i32,
    lit_length_freq: [u32; 36],
    lit_length_sum: u32,
    lit_length_sum_base_price: i32,
    match_length_freq: [u32; 53],
    match_length_sum: u32,
    match_length_sum_base_price: i32,
    off_code_freq: [u32; 32],
    off_code_sum: u32,
    off_code_sum_base_price: i32,
    price_type_predef: bool,

    // Scratch tables.
    price_table: Vec<Optimal>,
    match_table: Vec<OptMatch>,

    // Binary-tree match finder.
    mf: BtFinder,

    // Parameters.
    target_length: u32,
    opt_level: i32,
    two_pass_seeded: bool,
}

/// Binary-tree match finder state (`ms` in the C).
struct BtFinder {
    hash_table: Vec<u32>,
    hash_log: u32,
    bt: Vec<u32>,
    bt_log: u32,
    next_to_update: u32,
    hash3_table: Vec<u32>,
    hash_log3: u32,
    next_to_update3: u32,
    search_log: u32,
    window_log: u32,
    min_match: u32,
}

impl OptState {
    /// Build the opt state for a level's parameters. `src_len` is the
    /// full input length, used to clamp table sizes like the C's
    /// `ZSTD_adjustCParams_internal`.
    #[must_use]
    pub fn new(params: &super::cparams::CompressionParams, src_len: usize, opt_level: i32) -> Self {
        let src_log = if src_len <= 1 {
            1
        } else {
            usize::BITS - (src_len - 1).leading_zeros()
        };
        let window_log = params.window_log.min(src_log + 1);
        let chain_log = params.chain_log.min(window_log + 1);
        let hash_log = params.hash_log.min(window_log);
        let bt_log = (chain_log - 1).max(1);
        let hash_log3 = if params.min_match == 3 {
            HASHLOG3_MAX.min(window_log)
        } else {
            0
        };
        let min_match = if params.min_match == 3 { 3 } else { 4 };
        Self {
            lit_freq: [0; 256],
            lit_sum: 0,
            lit_sum_base_price: 0,
            lit_length_freq: [0; 36],
            lit_length_sum: 0,
            lit_length_sum_base_price: 0,
            match_length_freq: [0; 53],
            match_length_sum: 0,
            match_length_sum_base_price: 0,
            off_code_freq: [0; 32],
            off_code_sum: 0,
            off_code_sum_base_price: 0,
            price_type_predef: false,
            price_table: vec![Optimal::default(); OPT_SIZE],
            match_table: vec![OptMatch::default(); OPT_SIZE],
            mf: BtFinder {
                hash_table: vec![0; 1usize << hash_log],
                hash_log,
                bt: vec![0; 2 * (1usize << bt_log)],
                bt_log,
                next_to_update: 0,
                hash3_table: if hash_log3 > 0 {
                    vec![0; 1usize << hash_log3]
                } else {
                    Vec::new()
                },
                hash_log3,
                next_to_update3: 0,
                search_log: params.search_log,
                window_log,
                min_match,
            },
            target_length: params.target_length,
            opt_level,
            two_pass_seeded: false,
        }
    }

    fn set_base_prices(&mut self) {
        self.lit_sum_base_price = weight(self.lit_sum, self.opt_level);
        self.lit_length_sum_base_price = weight(self.lit_length_sum, self.opt_level);
        self.match_length_sum_base_price = weight(self.match_length_sum, self.opt_level);
        self.off_code_sum_base_price = weight(self.off_code_sum, self.opt_level);
    }

    fn downscale_stats(table: &mut [u32], shift: u32, base1: bool) -> u32 {
        let mut sum: u32 = 0;
        for t in table.iter_mut() {
            let base = if base1 { 1 } else { u32::from(*t > 0) };
            let new_stat = base + (*t >> shift);
            sum = sum.wrapping_add(new_stat);
            *t = new_stat;
        }
        sum
    }

    fn scale_stats(table: &mut [u32], log_target: u32) -> u32 {
        let prevsum: u32 = table.iter().sum();
        let factor = prevsum >> log_target;
        if factor <= 1 {
            return prevsum;
        }
        Self::downscale_stats(table, highbit32(factor), true)
    }

    /// `ZSTD_rescaleFreqs`. First block: seed from the block's own
    /// bytes; later blocks: downscale accumulated stats.
    fn rescale_freqs(&mut self, block: &[u8]) {
        self.price_type_predef = false;
        if self.lit_length_sum == 0 {
            if block.len() <= PREDEF_THRESHOLD {
                self.price_type_predef = true;
            }
            // Literals: raw histogram downscaled by 2^8.
            for b in block {
                self.lit_freq[*b as usize] += 1;
            }
            self.lit_sum = Self::downscale_stats(&mut self.lit_freq, 8, false);

            self.lit_length_freq = BASE_LL_FREQS;
            self.lit_length_sum = BASE_LL_FREQS.iter().sum();

            self.match_length_freq = [1; 53];
            self.match_length_sum = 53;

            self.off_code_freq = BASE_OF_FREQS;
            self.off_code_sum = BASE_OF_FREQS.iter().sum();
        } else {
            self.lit_sum = Self::scale_stats(&mut self.lit_freq, 12);
            self.lit_length_sum = Self::scale_stats(&mut self.lit_length_freq, 11);
            self.match_length_sum = Self::scale_stats(&mut self.match_length_freq, 11);
            self.off_code_sum = Self::scale_stats(&mut self.off_code_freq, 11);
        }
        self.set_base_prices();
    }

    /// `ZSTD_updateStats` after a stored sequence.
    fn update_stats(&mut self, literals: &[u8], off_base: u32, match_length: u32) {
        for &lit in literals {
            self.lit_freq[lit as usize] += LITFREQ_ADD;
        }
        self.lit_sum += (literals.len() as u32) * LITFREQ_ADD;

        let (ll_code_idx, _) = ll_code(literals.len() as u32);
        self.lit_length_freq[ll_code_idx as usize] += 1;
        self.lit_length_sum += 1;

        let off_code = highbit32(off_base);
        self.off_code_freq[off_code as usize] += 1;
        self.off_code_sum += 1;

        let (ml_code_idx, _) = ml_code(match_length);
        self.match_length_freq[ml_code_idx as usize] += 1;
        self.match_length_sum += 1;
    }
}

impl BtFinder {
    fn hash_ptr(&self, src: &[u8], pos: usize) -> usize {
        // mls >= 4 tree hash; mls == 3 handled by hash3 below.
        if self.min_match == 3 {
            let v = u32::from_le_bytes([
                src[pos],
                src[pos + 1],
                src[pos + 2],
                if pos + 3 < src.len() { src[pos + 3] } else { 0 },
            ]) & 0x00FF_FFFF;
            ((v.wrapping_mul(506_832_829) >> (32 - self.hash_log)) as usize)
                & ((1usize << self.hash_log) - 1)
        } else {
            let v = u32::from_le_bytes([src[pos], src[pos + 1], src[pos + 2], src[pos + 3]]);
            ((v.wrapping_mul(2_654_435_761) >> (32 - self.hash_log)) as usize)
                & ((1usize << self.hash_log) - 1)
        }
    }

    fn hash3_ptr(&self, src: &[u8], pos: usize) -> usize {
        let v = u32::from_le_bytes([
            src[pos],
            src[pos + 1],
            src[pos + 2],
            if pos + 3 < src.len() { src[pos + 3] } else { 0 },
        ]) & 0x00FF_FFFF;
        ((v.wrapping_mul(506_832_829) >> (32 - self.hash_log3)) as usize)
            & ((1usize << self.hash_log3) - 1)
    }
}

/// Count matching bytes between `src[a + k]` and `src[b + k]`, up to
/// `limit` bytes (positions are absolute in `src`). `a > b`.
fn count_abs(src: &[u8], a: usize, b: usize, limit: usize) -> usize {
    let mut len = 0usize;
    let max = limit.min(src.len() - a);
    while len + 8 <= max {
        let wa = u64::from_le_bytes(src[a + len..a + len + 8].try_into().unwrap());
        let wb = u64::from_le_bytes(src[b + len..b + len + 8].try_into().unwrap());
        let diff = wa ^ wb;
        if diff != 0 {
            return len + (diff.trailing_zeros() as usize / 8);
        }
        len += 8;
    }
    if len + 4 <= max {
        let wa = u32::from_le_bytes(src[a + len..a + len + 4].try_into().unwrap());
        let wb = u32::from_le_bytes(src[b + len..b + len + 4].try_into().unwrap());
        if wa != wb {
            return len + ((wa ^ wb).trailing_zeros() as usize / 8);
        }
        len += 4;
    }
    while len < max && src[a + len] == src[b + len] {
        len += 1;
    }
    len
}

/// Read `min_match` (3 or 4) bytes at `pos` as a comparable value.
fn read_minmatch(src: &[u8], pos: usize, min_match: u32) -> u32 {
    let v = u32::from_le_bytes([
        src[pos],
        src[pos + 1],
        src[pos + 2],
        if pos + 3 < src.len() { src[pos + 3] } else { 0 },
    ]);
    if min_match == 3 {
        v << 8
    } else {
        v
    }
}

/// `ZSTD_insertBt1`: insert position `pos` into the binary tree.
/// Returns the number of positions that can be skipped forward.
#[allow(clippy::too_many_lines)]
fn insert_bt1(mf: &mut BtFinder, src: &[u8], pos: usize, target: usize) -> u32 {
    let _ = target;
    let h = mf.hash_ptr(src, pos);
    let mut match_index = mf.hash_table[h] as usize;
    mf.hash_table[h] = pos as u32;

    let bt_log = mf.bt_log;
    let bt_mask = (1usize << bt_log) - 1;
    let curr = pos;
    let bt_floor = if bt_mask >= curr { 0 } else { curr - bt_mask };
    let window_low = curr.saturating_sub(1usize << mf.window_log).max(1);
    let mut smaller_ptr: Option<usize> = Some(2 * (curr & bt_mask));
    let mut larger_ptr: Option<usize> = Some(2 * (curr & bt_mask) + 1);
    let mut common_length_smaller = 0usize;
    let mut common_length_larger = 0usize;
    let mut match_end_idx = curr + 9;
    let mut best_length = 8usize;
    let mut nb_compares = 1usize << mf.search_log;

    while nb_compares > 0 && match_index >= window_low {
        nb_compares -= 1;
        let next_small = 2 * (match_index & bt_mask);
        let next_large = next_small + 1;
        // Guaranteed minimum common length with prior comparisons.
        let mut match_length = common_length_smaller.min(common_length_larger);
        debug_assert!(match_index < curr);
        if pos + match_length < src.len() {
            match_length += count_abs(
                src,
                pos + match_length,
                match_index + match_length,
                src.len() - pos - match_length,
            );
        }

        if match_length > best_length {
            best_length = match_length;
            if match_length > match_end_idx - match_index {
                match_end_idx = match_index + match_length;
            }
        }

        if pos + match_length == src.len() {
            // Equal to end: cannot order in the tree; drop to keep it
            // consistent (misses a little compression).
            break;
        }

        let mismatch_a = src[match_index + match_length];
        let mismatch_b = src[pos + match_length];
        if mismatch_a < mismatch_b {
            // match is smaller than current.
            if let Some(sp) = smaller_ptr {
                mf.bt[sp] = match_index as u32;
            }
            common_length_smaller = match_length;
            if match_index <= bt_floor {
                smaller_ptr = None;
                break;
            }
            smaller_ptr = Some(next_large);
            match_index = mf.bt[next_large] as usize;
        } else {
            if let Some(lp) = larger_ptr {
                mf.bt[lp] = match_index as u32;
            }
            common_length_larger = match_length;
            if match_index <= bt_floor {
                larger_ptr = None;
                break;
            }
            larger_ptr = Some(next_small);
            match_index = mf.bt[next_small] as usize;
        }
    }

    if let Some(sp) = smaller_ptr {
        mf.bt[sp] = 0;
    }
    if let Some(lp) = larger_ptr {
        mf.bt[lp] = 0;
    }
    let positions = if best_length > 384 {
        192usize.min(best_length - 384)
    } else {
        0
    };
    debug_assert!(match_end_idx > curr + 8);
    (positions.max(match_end_idx - (curr + 8))) as u32
}

/// `ZSTD_updateTree_internal`: fill the tree up to `target`.
fn update_tree(mf: &mut BtFinder, src: &[u8], target: usize) {
    // Tree insertion needs 8 readable bytes.
    let target = target.min(src.len().saturating_sub(8));
    let mut idx = mf.next_to_update as usize;
    while idx < target {
        let forward = insert_bt1(mf, src, idx, target);
        debug_assert!(forward > 0);
        idx += forward as usize;
    }
    mf.next_to_update = target as u32;
}

/// `ZSTD_insertAndFindFirstIndexHash3`.
fn insert_and_find_first_index_hash3(mf: &mut BtFinder, src: &[u8], pos: usize) -> usize {
    let mut idx = mf.next_to_update3 as usize;
    let target = pos;
    while idx < target {
        let h = mf.hash3_ptr(src, idx);
        mf.hash3_table[h] = idx as u32;
        idx += 1;
    }
    mf.next_to_update3 = target as u32;
    mf.hash3_table[mf.hash3_ptr(src, pos)] as usize
}

/// `ZSTD_insertBtAndGetAllMatches`. Fills `st.match_table`, returns
/// the match count. `ip`/`rep`/`ll0`/`length_to_beat` as in the C.
#[allow(clippy::too_many_lines)]
fn insert_bt_and_get_all_matches(
    mf: &mut BtFinder,
    match_buf: &mut [OptMatch],
    src: &[u8],
    ip: usize,
    rep: &[u32; 3],
    ll0: u32,
    length_to_beat: u32,
) -> usize {
    if (ip as u32) < mf.next_to_update {
        return 0; // skipped area
    }
    update_tree(mf, src, ip);
    let sufficient_len = OPT_NUM as u32 - 1;
    let curr = ip;
    let min_match = if mf.min_match == 3 { 3 } else { 4 };
    let window_low = curr.saturating_sub(1usize << mf.window_log).max(1);
    let match_low = window_low.max(1);
    let bt_log = mf.bt_log;
    let bt_mask = (1usize << bt_log) - 1;
    let bt_floor = if bt_mask >= curr { 0 } else { curr - bt_mask };
    let mut mnum = 0usize;
    let mut nb_compares = 1usize << mf.search_log;
    let mut best_length = (length_to_beat - 1) as usize;
    let mut match_end_idx = curr + 9;

    // Check repcodes.
    {
        let last_r = REP_NUM + ll0 as usize;
        for rep_code in (ll0 as usize)..last_r {
            let rep_offset = if rep_code == REP_NUM {
                rep[0].wrapping_sub(1)
            } else {
                rep[rep_code]
            };
            let rep_index = curr.wrapping_sub(rep_offset as usize);
            // Discard offsets 0 and positions outside the window.
            if rep_offset == 0 || rep_offset as usize > curr || rep_index < window_low {
                continue;
            }
            if read_minmatch(src, ip, min_match)
                != read_minmatch(src, ip - rep_offset as usize, min_match)
            {
                continue;
            }
            let rep_len = count_abs(
                src,
                ip + min_match as usize,
                ip + min_match as usize - rep_offset as usize,
                src.len() - ip - min_match as usize,
            ) + min_match as usize;
            if rep_len > best_length {
                best_length = rep_len;
                match_buf[mnum] = OptMatch {
                    off: (rep_code - ll0 as usize + 1) as u32,
                    len: rep_len as u32,
                };
                mnum += 1;
                if rep_len as u32 > sufficient_len || ip + rep_len == src.len() {
                    return mnum;
                }
            }
        }
    }

    // HC3 (len-3) match finder.
    if mf.min_match == 3 && best_length < min_match as usize {
        let match_index3 = insert_and_find_first_index_hash3(mf, src, ip);
        if match_index3 >= match_low && curr - match_index3 < (1 << 18) {
            let mlen = count_abs(src, ip, match_index3, src.len() - ip);
            if mlen >= min_match as usize {
                best_length = mlen;
                debug_assert!(mnum == 0);
                match_buf[0] = OptMatch {
                    off: offset_to_offbase((curr - match_index3) as u32),
                    len: mlen as u32,
                };
                mnum = 1;
                if mlen as u32 > sufficient_len || ip + mlen == src.len() {
                    mf.next_to_update = curr as u32 + 1;
                    return 1;
                }
            }
        }
    }

    // Tree walk.
    let h = mf.hash_ptr(src, ip);
    let mut match_index = mf.hash_table[h] as usize;
    mf.hash_table[h] = ip as u32;

    let mut smaller_ptr: Option<usize> = Some(2 * (curr & bt_mask));
    let mut larger_ptr: Option<usize> = Some(2 * (curr & bt_mask) + 1);
    let mut common_length_smaller = 0usize;
    let mut common_length_larger = 0usize;

    while nb_compares > 0 && match_index >= match_low {
        nb_compares -= 1;
        let next_small = 2 * (match_index & bt_mask);
        let next_large = next_small + 1;
        let mut match_length = common_length_smaller.min(common_length_larger);
        debug_assert!(curr > match_index);
        if ip + match_length < src.len() {
            match_length += count_abs(
                src,
                ip + match_length,
                match_index + match_length,
                src.len() - ip - match_length,
            );
        }

        if match_length > best_length {
            if match_length > match_end_idx - match_index {
                match_end_idx = match_index + match_length;
            }
            best_length = match_length;
            match_buf[mnum] = OptMatch {
                off: offset_to_offbase((curr - match_index) as u32),
                len: match_length as u32,
            };
            mnum += 1;
            if match_length > OPT_NUM || ip + match_length == src.len() {
                break;
            }
        }

        let mismatch_a = src[match_index + match_length];
        let mismatch_b = src[ip + match_length];
        if mismatch_a < mismatch_b {
            if let Some(sp) = smaller_ptr {
                mf.bt[sp] = match_index as u32;
            }
            common_length_smaller = match_length;
            if match_index <= bt_floor {
                smaller_ptr = None;
                break;
            }
            smaller_ptr = Some(next_large);
            match_index = mf.bt[next_large] as usize;
        } else {
            if let Some(lp) = larger_ptr {
                mf.bt[lp] = match_index as u32;
            }
            common_length_larger = match_length;
            if match_index <= bt_floor {
                larger_ptr = None;
                break;
            }
            larger_ptr = Some(next_small);
            match_index = mf.bt[next_small] as usize;
        }
    }

    if let Some(sp) = smaller_ptr {
        mf.bt[sp] = 0;
    }
    if let Some(lp) = larger_ptr {
        mf.bt[lp] = 0;
    }

    mf.next_to_update = (match_end_idx - 8) as u32;
    if std::env::var_os("ZSTD_OPT_DUMP").is_some() && (curr as u32) == 33 {
        eprintln!(
            "OPTDUMP ip=33 mnum={mnum} ll0={ll0} reps={:?} cands={:?}",
            &rep.iter().collect::<Vec<_>>(),
            &match_buf[..mnum]
                .iter()
                .map(|m| (m.off, m.len))
                .collect::<Vec<_>>()
        );
    }
    mnum
}

/// `ZSTD_compressBlock_opt_generic`. Parses `src[prefix_len..]` and
/// appends sequences to `seq_store` (which carries the incoming rep
/// offsets in `rep_offsets` and is kept updated).
#[allow(clippy::too_many_lines)]
pub fn compress_block_opt_with_prefix(
    src: &[u8],
    prefix_len: usize,
    seq_store: &mut SeqStore,
    st: &mut OptState,
) -> usize {
    let istart = prefix_len;
    let iend = src.len();
    let min_match = if st.mf.min_match == 3 { 3 } else { 4 };
    // The C uses targetLength (16 at L8-11) purely as a SPEED knob to
    // cut DP exploration; at 16 it fragments long matches on tiny
    // repetitive inputs (L9 worse than L1 measured). Floor at the
    // L12+ value.
    let sufficient_len = st.target_length.clamp(32, OPT_NUM as u32 - 1);

    if iend < istart + min_match + 8 {
        seq_store.literals.extend_from_slice(&src[istart..]);
        return iend - istart;
    }

    let ilimit = iend - 8;

    // btultra2 two-pass seeding on the first block of the frame.
    if st.opt_level == 2
        && !st.two_pass_seeded
        && st.lit_length_sum == 0
        && prefix_len == 0
        && seq_store.sequences.is_empty()
        && seq_store.literals.is_empty()
        && iend - istart > PREDEF_THRESHOLD
    {
        st.two_pass_seeded = true;
        let mut scratch = SeqStore::new();
        scratch.reset(seq_store.rep_offsets);
        compress_block_opt_with_prefix(src, prefix_len, &mut scratch, st);
        // Keep only the frequency statistics: reset the match tables
        // so the second pass re-inserts from scratch (equivalent to
        // the C's window-invalidation trick).
        st.mf.hash_table.fill(0);
        st.mf.bt.fill(0);
        st.mf.next_to_update = 0;
        st.mf.hash3_table.fill(0);
        st.mf.next_to_update3 = 0;
    }

    let mut rep = seq_store.rep_offsets;
    st.rescale_freqs(&src[istart..iend]);
    let mut ip = if istart == 0 { 1 } else { istart };
    let mut anchor = istart;

    while ip < ilimit {
        let litlen = (ip - anchor) as u32;
        let ll0 = u32::from(litlen == 0);
        let nb_matches = insert_bt_and_get_all_matches(
            &mut st.mf,
            &mut st.match_table,
            src,
            ip,
            &rep,
            ll0,
            min_match as u32,
        );
        if nb_matches == 0 {
            ip += 1;
            continue;
        }

        // Initialize opt[0].
        {
            let pr = prices_of!(st);
            let opt = &mut st.price_table;
            opt[0].mlen = 0;
            opt[0].litlen = litlen;
            opt[0].price = pr.lit_length_price(litlen);
            opt[0].rep = rep;
        }

        let mut cur;
        let mut last_pos;

        // Large match: immediate encoding.
        let max_ml = st.match_table[nb_matches - 1].len;
        let max_off_base = st.match_table[nb_matches - 1].off;
        let mut last_stretch = Optimal::default();
        if max_ml > sufficient_len {
            last_stretch.litlen = 0;
            last_stretch.mlen = max_ml;
            last_stretch.off = max_off_base;
            cur = 0;
            last_pos = max_ml;
            // Jump to the shortest-path phase.
            if let Some(res) = shortest_path(
                src,
                ip,
                anchor,
                st,
                seq_store,
                &mut rep,
                cur,
                last_pos,
                last_stretch,
            ) {
                let (new_ip, new_anchor) = res;
                ip = new_ip;
                anchor = new_anchor;
            }
            continue;
        }

        // Set prices for first matches at stretch position == 0.
        {
            let pr = prices_of!(st);
            let opt = &mut st.price_table;
            let mut pos = 1usize;
            while pos < min_match as usize {
                opt[pos].price = MAX_PRICE;
                opt[pos].mlen = 0;
                opt[pos].litlen = litlen + pos as u32;
                pos += 1;
            }
            for match_nb in 0..nb_matches {
                let off_base = st.match_table[match_nb].off;
                let end = st.match_table[match_nb].len as usize;
                while pos <= end {
                    let match_price = pr.get_match_price(off_base, pos as u32);
                    let sequence_price = opt[0].price + match_price;
                    opt[pos].mlen = pos as u32;
                    opt[pos].off = off_base;
                    opt[pos].litlen = 0;
                    opt[pos].price = sequence_price + pr.lit_length_price(0);
                    pos += 1;
                }
            }
            last_pos = pos as u32 - 1;
            opt[pos].price = MAX_PRICE;
        }

        // Check further positions.
        cur = 1;
        while cur <= last_pos {
            let inr = ip + cur as usize;

            // Fix current position with one literal if cheaper.
            {
                let pr = prices_of!(st);
                let opt = &mut st.price_table;
                let litlen = opt[cur as usize - 1].litlen + 1;
                let price = opt[cur as usize - 1].price
                    + pr.raw_literals_cost(&src[inr - 1..inr])
                    + (pr.lit_length_price(litlen) - pr.lit_length_price(litlen - 1));
                if price <= opt[cur as usize].price {
                    let prev_match = opt[cur as usize];
                    opt[cur as usize] = opt[cur as usize - 1];
                    opt[cur as usize].litlen = litlen;
                    opt[cur as usize].price = price;
                    if st.opt_level >= 1
                        && prev_match.litlen == 0
                        && (pr.lit_length_price(1) - pr.lit_length_price(0)) < 0
                        && (ip + cur as usize) < iend
                    {
                        // Check next position: match+1 literal may beat
                        // a longer literals run.
                        let with_1_literal = prev_match.price
                            + pr.raw_literals_cost(&src[inr..inr + 1])
                            + (pr.lit_length_price(1) - pr.lit_length_price(0));
                        let with_more_literals = price
                            + pr.raw_literals_cost(&src[inr..inr + 1])
                            + (pr.lit_length_price(litlen + 1) - pr.lit_length_price(litlen));
                        if with_1_literal < with_more_literals
                            && with_1_literal < opt[cur as usize + 1].price
                        {
                            let prev = cur as usize - prev_match.mlen as usize;
                            let ll0_prev = u32::from(opt[prev].litlen == 0);
                            let new_reps = new_rep(&opt[prev].rep, prev_match.off, ll0_prev);
                            opt[cur as usize + 1] = prev_match;
                            opt[cur as usize + 1].rep = new_reps;
                            opt[cur as usize + 1].litlen = 1;
                            opt[cur as usize + 1].price = with_1_literal;
                            if last_pos < cur + 1 {
                                last_pos = cur + 1;
                            }
                        }
                    }
                }
            }

            // Offset history update for a confirmed match end.
            {
                let opt = &mut st.price_table;
                debug_assert!(cur as usize >= opt[cur as usize].mlen as usize);
                if opt[cur as usize].litlen == 0 {
                    let prev = cur as usize - opt[cur as usize].mlen as usize;
                    let ll0_prev = u32::from(opt[prev].litlen == 0);
                    let new_reps = new_rep(&opt[prev].rep, opt[cur as usize].off, ll0_prev);
                    opt[cur as usize].rep = new_reps;
                }
            }

            if inr > ilimit {
                cur += 1;
                continue;
            }
            if cur == last_pos {
                break;
            }

            if st.opt_level == 0 && {
                let opt = &st.price_table;
                opt[cur as usize + 1].price <= opt[cur as usize].price + (BITCOST_MULTIPLIER / 2)
            } {
                cur += 1;
                continue;
            }

            {
                let pr = prices_of!(st);
                let (cur_price, cur_litlen, cur_rep) = {
                    let opt = &st.price_table;
                    (
                        opt[cur as usize].price,
                        opt[cur as usize].litlen,
                        opt[cur as usize].rep,
                    )
                };
                let ll0 = u32::from(cur_litlen == 0);
                let base_price = cur_price + pr.lit_length_price(0);
                let nb_matches = insert_bt_and_get_all_matches(
                    &mut st.mf,
                    &mut st.match_table,
                    src,
                    inr,
                    &cur_rep,
                    ll0,
                    min_match as u32,
                );
                if nb_matches == 0 {
                    cur += 1;
                    continue;
                }
                let matches = &st.match_table;
                let longest_ml = matches[nb_matches - 1].len;

                if longest_ml > sufficient_len
                    || cur + longest_ml >= OPT_NUM as u32
                    || ip + cur as usize + longest_ml as usize >= iend
                {
                    last_stretch.mlen = longest_ml;
                    last_stretch.off = matches[nb_matches - 1].off;
                    last_stretch.litlen = 0;
                    last_pos = cur + longest_ml;
                    cur = last_pos - last_stretch.mlen;
                    if let Some(res) = shortest_path(
                        src,
                        ip,
                        anchor,
                        st,
                        seq_store,
                        &mut rep,
                        cur,
                        last_pos,
                        last_stretch,
                    ) {
                        let (new_ip, new_anchor) = res;
                        ip = new_ip;
                        anchor = new_anchor;
                    }
                    break;
                }

                // Set prices using matches found at position == cur.
                let pr = prices_of!(st);
                let opt = &mut st.price_table;
                for match_nb in 0..nb_matches {
                    let offset = matches[match_nb].off;
                    let last_ml = matches[match_nb].len;
                    let start_ml = if match_nb > 0 {
                        matches[match_nb - 1].len + 1
                    } else {
                        min_match as u32
                    };
                    let mut mlen = last_ml;
                    while mlen >= start_ml {
                        let pos = cur as usize + mlen as usize;
                        let price = base_price + pr.get_match_price(offset, mlen);
                        if pos as u32 > last_pos || price < opt[pos].price {
                            while last_pos < pos as u32 {
                                last_pos += 1;
                                opt[last_pos as usize].price = MAX_PRICE;
                                opt[last_pos as usize].litlen = 1; // "not end of match"
                            }
                            opt[pos].mlen = mlen;
                            opt[pos].off = offset;
                            opt[pos].litlen = 0;
                            opt[pos].price = price;
                        } else if st.opt_level == 0 {
                            break; // early abort
                        }
                        mlen -= 1;
                    }
                }
                opt[last_pos as usize + 1].price = MAX_PRICE;
            }
            cur += 1;
        } // while cur <= last_pos

        // Series ended without a forced shortest-path jump.
        if last_stretch.mlen == 0 && last_stretch.litlen == 0 {
            let opt = &st.price_table;
            last_stretch = opt[last_pos as usize];
            debug_assert!(last_pos >= last_stretch.mlen);
            cur = last_pos - last_stretch.mlen;
            if let Some(res) = shortest_path(
                src,
                ip,
                anchor,
                st,
                seq_store,
                &mut rep,
                cur,
                last_pos,
                last_stretch,
            ) {
                let (new_ip, new_anchor) = res;
                ip = new_ip;
                anchor = new_anchor;
            }
        }
    } // while ip < ilimit

    if anchor < iend {
        seq_store.literals.extend_from_slice(&src[anchor..iend]);
    }
    seq_store.rep_offsets = rep;
    iend - anchor
}

/// `ZSTD_newRep`.
fn new_rep(rep: &[u32; 3], off_base: u32, ll0: u32) -> [u32; 3] {
    let mut new_reps = *rep;
    if off_base > REP_NUM as u32 {
        update_rep_by_offset(&mut new_reps, off_base - REP_NUM as u32);
    } else {
        let rep_code = off_base - 1 + ll0;
        if rep_code > 0 {
            let current_offset = if rep_code == REP_NUM as u32 {
                rep[0].wrapping_sub(1)
            } else {
                rep[rep_code as usize]
            };
            new_reps[2] = if rep_code >= 2 { rep[1] } else { rep[2] };
            new_reps[1] = rep[0];
            new_reps[0] = current_offset;
        }
    }
    new_reps
}

/// Resolve an offBase chosen by the parser into the real byte offset
/// it denotes, given the offset history in effect at the sequence.
fn resolve_off_base(off_base: u32, rep: &[u32; 3], ll0: u32) -> u32 {
    if off_base > REP_NUM as u32 {
        return off_base - REP_NUM as u32;
    }
    let rep_code = off_base - 1 + ll0;
    if rep_code == REP_NUM as u32 {
        rep[0].wrapping_sub(1)
    } else {
        rep[rep_code as usize]
    }
}

/// The `_shortestPath` phase: reverse-traverse `opt` from
/// (`last_pos`, `last_stretch`) and store the selected sequences.
/// Returns the new (ip, anchor).
fn shortest_path(
    src: &[u8],
    ip: usize,
    anchor: usize,
    st: &mut OptState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    cur: u32,
    last_pos: u32,
    last_stretch: Optimal,
) -> Option<(usize, usize)> {
    debug_assert!(st.price_table[0].mlen == 0);
    debug_assert!(last_pos >= last_stretch.mlen);
    debug_assert!(cur == last_pos - last_stretch.mlen);

    if last_stretch.mlen == 0 {
        // No solution: all matches converted into literals.
        debug_assert!((ip - anchor) as u32 + last_pos == last_stretch.litlen);
        return Some((ip + last_pos as usize, anchor));
    }
    debug_assert!(last_stretch.off > 0);

    let mut cur = cur as usize;
    // The rep state the decoder holds when this series begins; every
    // stored sequence must be resolved against a forward walk from
    // here (the parser-side update below must not leak into it).
    let rep_entry = *rep;
    // Rep-state walk replaces the C's pre-store history update: the
    // forward walk below applies the same rotations in order and
    // yields both the parser's next-series state and the store's.
    let mut out_ip = ip;
    let out_anchor = anchor;
    if last_stretch.litlen != 0 {
        debug_assert!(cur >= last_stretch.litlen as usize);
        cur -= last_stretch.litlen as usize;
    }
    // cur0 is the post-litlen-adjustment start (the C computes
    // storeEnd = cur + 2 only AFTER subtracting lastStretch.litlen).
    let cur0 = cur;

    // Reverse traversal: collect the sequence list locally (the C
    // overwrites opt in reverse; a local list is equivalent and lets
    // the price-table borrow end before update_stats runs).
    let store_end = cur0 + 2;
    debug_assert!(store_end < OPT_SIZE);
    let opt = &mut st.price_table;
    opt[store_end] = last_stretch;
    let mut store_start = store_end;
    let mut stretch_pos = cur0;
    loop {
        let next_stretch = opt[stretch_pos];
        opt[store_start].litlen = next_stretch.litlen;
        if next_stretch.mlen == 0 {
            break;
        }
        debug_assert!(store_start > 0);
        store_start -= 1;
        opt[store_start] = next_stretch;
        debug_assert!(next_stretch.litlen as usize + next_stretch.mlen as usize <= stretch_pos);
        stretch_pos -= next_stretch.litlen as usize + next_stretch.mlen as usize;
    }
    let seqs: Vec<(u32, u32, u32)> = (store_start..=store_end)
        .map(|i| (opt[i].litlen, opt[i].mlen, opt[i].off))
        .collect();

    // Store sequences, resolving offBase to real offsets against a
    // forward walk of the rep history from the series entry state.
    let mut new_anchor = out_anchor;
    let mut walk_rep = rep_entry;
    for store_pos in store_start..=store_end {
        let (llen, mlen, off_base) = seqs[store_pos - store_start];

        if mlen == 0 {
            // Only literals: must be the last entry; starts a new
            // series without advancing the anchor.
            debug_assert!(store_pos == store_end);
            out_ip = new_anchor + llen as usize;
            continue;
        }

        debug_assert!(new_anchor + llen as usize <= src.len());
        let literals = &src[new_anchor..new_anchor + llen as usize];
        st.update_stats(literals, off_base, mlen);

        let ll0 = u32::from(llen == 0);
        let real_offset = resolve_off_base(off_base, &walk_rep, ll0);
        debug_assert!(real_offset > 0);
        debug_assert!(real_offset as usize <= new_anchor + llen as usize);
        seq_store.literals.extend_from_slice(literals);
        seq_store.sequences.push(RawSequence {
            literal_length: llen,
            match_length: mlen,
            offset: real_offset,
        });
        rotate_reps(&mut seq_store.rep_offsets, real_offset);
        walk_rep = new_rep(&walk_rep, off_base, ll0);

        let advance = (llen + mlen) as usize;
        new_anchor += advance;
        out_ip = new_anchor;
    }

    st.set_base_prices();
    *rep = walk_rep;
    seq_store.rep_offsets = walk_rep;
    Some((out_ip, new_anchor))
}
