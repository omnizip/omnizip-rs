//! Optimal parser with trained probability prices — a line-by-line
//! port of xz's `lzma_encoder_optimum_normal.c` (Igor Pavlov's
//! algorithm, 0BSD). Replaces the heuristic-priced DP in
//! [`super::optimal`]: prices come from the encoder's live
//! probability models, refreshed on the same counters the C encoder
//! uses, so the parse tracks what the models actually cost as they
//! train.
//!
//! The driver keeps its own match-finder position bookkeeping
//! (`read_pos` / `read_ahead`, mirroring `lzma_mf`) and pulls
//! candidate ladders from the windowed BT4 finder
//! ([`omnizip_codecs::Bt4MatchFinder`]) — the same finder the
//! reference uses for all NORMAL-mode presets.

#![forbid(unsafe_code)]
// Line-by-line C port: helper1/helper2 keep the original's shape
// (long functions, many parameters, index loops) so the translation
// stays diffable against lzma_encoder_optimum_normal.c.
#![allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::manual_memcpy,
    clippy::similar_names
)]

use crate::coder::distance_encoder::distance_slot;
use crate::encoder::lzma1::Lzma1Encoder;
use crate::range_coder::price::{
    rc_bit_0_price, rc_bit_1_price, rc_bit_price, rc_bittree_price, rc_bittree_reverse_price,
    rc_direct_price,
};
use crate::state::LzmaState;
use omnizip_codecs::Bt4MatchFinder;

const REPS: usize = 4;
const OPTS: usize = 1 << 12;
const MATCH_LEN_MIN: u32 = 2;
const MATCH_LEN_MAX: u32 = 273;
const DIST_MODEL_START: u32 = 4;
const DIST_MODEL_END: u32 = 14;
const FULL_DISTANCES: usize = 128;
const ALIGN_SIZE: usize = 16;
const ALIGN_MASK: u32 = 15;
const RC_INFINITY_PRICE: u32 = 1 << 30;
const DIST_SLOTS: usize = 64;

/// Match candidate (`dist` is 0-based, like xz's `lzma_match`).
#[derive(Clone, Copy)]
struct MatchC {
    len: u32,
    dist: u32,
}

#[derive(Clone)]
struct Optimal {
    state: u8,
    prev_1_is_literal: bool,
    prev_2: bool,
    pos_prev_2: u32,
    back_prev_2: u32,
    price: u32,
    pos_prev: u32,
    back_prev: u32,
    backs: [u32; REPS],
}

impl Optimal {
    fn new() -> Self {
        Self {
            state: 0,
            prev_1_is_literal: false,
            prev_2: false,
            pos_prev_2: 0,
            back_prev_2: 0,
            price: RC_INFINITY_PRICE,
            pos_prev: 0,
            back_prev: u32::MAX,
            backs: [0; REPS],
        }
    }
}

/// Parse + price-cache state carried across `optimum_next_symbol`
/// calls (the `lzma_lzma1_encoder` fields that live outside the
/// probability models themselves).
pub struct OptimumState {
    read_pos: usize,
    read_ahead: u32,
    nice_len: u32,
    dict_size: u32,
    matches: Vec<MatchC>,
    longest_match_length: u32,
    opts: Vec<Optimal>,
    opts_end_index: usize,
    opts_current_index: usize,
    dist_slot_prices: Box<[[u32; DIST_SLOTS]; 4]>,
    dist_prices: Box<[[u32; FULL_DISTANCES]; 4]>,
    dist_table_size: usize,
    match_price_count: u32,
    align_prices: [u32; ALIGN_SIZE],
    align_price_count: u32,
    stats_file_pos: Option<u64>,
    ladder: Vec<(u32, u32)>,
}

/// `lzma_memcmplen` — count matching bytes between `a` and `b`,
/// starting at `start` and capped at `limit` (both absolute indices).
fn memcmplen(data: &[u8], a: usize, b: usize, start: u32, limit: u32) -> u32 {
    let mut len = start;
    // Compare 8 bytes at a time while both sides stay in bounds and
    // word-aligned stepping is possible (semantics identical to the
    // byte loop; word-at-a-time is how the C reference gets its speed).
    while len + 8 <= limit
        && data[a + len as usize..a + len as usize + 8]
            == data[b + len as usize..b + len as usize + 8]
    {
        len += 8;
    }
    while len < limit && data[a + len as usize] == data[b + len as usize] {
        len += 1;
    }
    len
}

fn not_equal_16(data: &[u8], a: usize, b: usize) -> bool {
    data[a] != data[b] || data[a + 1] != data[b + 1]
}

impl OptimumState {
    /// Construct parse state. `nice_len` / `dict_size` come from the
    /// encoder options; `pb` sizes the position-state dimension.
    #[must_use]
    pub fn new(nice_len: u32, dict_size: u32, _pb: u32) -> Self {
        Self {
            read_pos: 0,
            read_ahead: 0,
            nice_len: nice_len.clamp(4, MATCH_LEN_MAX),
            dict_size,
            matches: Vec::with_capacity(MATCH_LEN_MAX as usize + 1),
            longest_match_length: 0,
            opts: vec![Optimal::new(); OPTS],
            opts_end_index: 0,
            opts_current_index: 0,
            dist_slot_prices: Box::new([[0; DIST_SLOTS]; 4]),
            dist_prices: Box::new([[0; FULL_DISTANCES]; 4]),
            dist_table_size: 64,
            match_price_count: u32::MAX / 2,
            align_prices: [0; ALIGN_SIZE],
            align_price_count: u32::MAX / 2,
            stats_file_pos: std::env::var("OMNIZIP_SYMSTATS")
                .ok()
                .and_then(|v| v.parse().ok()),
            ladder: Vec::with_capacity(64),
        }
    }
}

/// Port of `helper1` — the first DP step at the decision position.
/// `pos` is the absolute decision position. Returns `len_end`, or
/// `None` when the symbol was decided greedily inside.
fn helper1(
    enc: &Lzma1Encoder,
    st: &mut OptimumState,
    input: &[u8],
    bt: &mut Bt4MatchFinder<'_>,
    position: u32,
    back_res: &mut u32,
    len_res: &mut u32,
) -> Option<u32> {
    let nice_len = st.nice_len;
    let len_main;
    let matches_count;

    if st.read_ahead == 0 {
        mf_find(enc, st, input, bt);
        len_main = st.longest_match_length;
        matches_count = st.matches.len();
    } else {
        len_main = st.longest_match_length;
        matches_count = st.matches.len();
    }
    let pos = st.read_pos - 1; // decision position (buf)
    let buf_avail_full = input.len() - pos;
    let buf_avail = (buf_avail_full as u32).min(MATCH_LEN_MAX);
    if buf_avail < 2 {
        *back_res = u32::MAX;
        *len_res = 1;
        return None;
    }

    let mut rep_lens = [0u32; REPS];
    let mut rep_max_index = 0usize;
    let reps: [u32; REPS] = [enc.rep0, enc.rep1, enc.rep2, enc.rep3];

    for i in 0..REPS {
        if reps[i] as usize >= pos || not_equal_16(input, pos, pos - reps[i] as usize - 1) {
            rep_lens[i] = 0;
            continue;
        }
        let back = pos - reps[i] as usize - 1;
        rep_lens[i] = memcmplen(input, pos, back, 2, buf_avail);
        if rep_lens[i] > rep_lens[rep_max_index] {
            rep_max_index = i;
        }
    }

    if rep_lens[rep_max_index] >= nice_len {
        *back_res = rep_max_index as u32;
        *len_res = rep_lens[rep_max_index];
        mf_skip(st, input, bt, *len_res - 1);
        return None;
    }

    if len_main >= nice_len {
        *back_res = st.matches[matches_count - 1].dist + REPS as u32;
        *len_res = len_main;
        mf_skip(st, input, bt, len_main - 1);
        return None;
    }

    let current_byte = input[pos];
    let match_byte = if (enc.rep0 as usize) < pos {
        input[pos - enc.rep0 as usize - 1]
    } else {
        0
    };

    if len_main < 2 && current_byte != match_byte && rep_lens[rep_max_index] < 2 {
        *back_res = u32::MAX;
        *len_res = 1;
        return None;
    }

    st.opts[0].state = enc.state.as_u8();

    let pos_state = position & enc.pb_mask;
    let state_idx = usize::from(enc.state.as_u8());
    let pos_states = 1usize << enc.pb as usize;
    let is_match_idx = state_idx * pos_states + pos_state as usize;

    st.opts[1].price = rc_bit_0_price(enc.is_match[is_match_idx].probability())
        + get_literal_price(
            enc,
            position,
            if pos > 0 { input[pos - 1] } else { 0 },
            !enc.state.is_literal_context(),
            match_byte,
            current_byte,
        );
    make_literal(&mut st.opts[1]);

    let match_price = rc_bit_1_price(enc.is_match[is_match_idx].probability());
    let rep_match_price = match_price + rc_bit_1_price(enc.is_rep[state_idx].probability());

    if match_byte == current_byte {
        let short_rep_price = rep_match_price + get_short_rep_price(enc, state_idx, is_match_idx);
        if short_rep_price < st.opts[1].price {
            st.opts[1].price = short_rep_price;
            make_short_rep(&mut st.opts[1]);
        }
    }

    let len_end = len_main.max(rep_lens[rep_max_index]);
    if len_end < 2 {
        *back_res = st.opts[1].back_prev;
        *len_res = 1;
        return None;
    }

    st.opts[1].pos_prev = 0;
    st.opts[0].backs = reps;

    for len in (2..=len_end).rev() {
        st.opts[len as usize].price = RC_INFINITY_PRICE;
    }

    for i in 0..REPS {
        let mut rep_len = rep_lens[i];
        if rep_len < 2 {
            continue;
        }
        let price = rep_match_price + get_pure_rep_price(enc, i as u32, state_idx, is_match_idx);
        loop {
            let cur_and_len_price = price
                + enc
                    .rep_length_encoder
                    .price(rep_len - MATCH_LEN_MIN, pos_state as usize);
            if cur_and_len_price < st.opts[rep_len as usize].price {
                st.opts[rep_len as usize].price = cur_and_len_price;
                st.opts[rep_len as usize].pos_prev = 0;
                st.opts[rep_len as usize].back_prev = i as u32;
                st.opts[rep_len as usize].prev_1_is_literal = false;
            }
            rep_len -= 1;
            if rep_len < 2 {
                break;
            }
        }
    }

    let normal_match_price = match_price + rc_bit_0_price(enc.is_rep[state_idx].probability());

    let mut len = if rep_lens[0] >= 2 { rep_lens[0] + 1 } else { 2 };
    if len <= len_main {
        let mut i = 0usize;
        while len > st.matches[i].len {
            i += 1;
        }
        loop {
            let dist = st.matches[i].dist;
            let cur_and_len_price =
                normal_match_price + get_dist_len_price(enc, st, dist, len, pos_state as usize);
            if cur_and_len_price < st.opts[len as usize].price {
                st.opts[len as usize].price = cur_and_len_price;
                st.opts[len as usize].pos_prev = 0;
                st.opts[len as usize].back_prev = dist + REPS as u32;
                st.opts[len as usize].prev_1_is_literal = false;
            }
            if len == st.matches[i].len {
                i += 1;
                if i == matches_count {
                    break;
                }
            }
            len += 1;
        }
    }

    Some(len_end)
}

fn make_literal(o: &mut Optimal) {
    o.back_prev = u32::MAX;
    o.prev_1_is_literal = false;
}

fn make_short_rep(o: &mut Optimal) {
    o.back_prev = 0;
    o.prev_1_is_literal = false;
}

fn is_short_rep(o: &Optimal) -> bool {
    o.back_prev == 0
}

/// Port of `helper2` — one DP step `cur` bytes past the decision
/// position. Returns the (possibly extended) `len_end`.
fn helper2(
    enc: &Lzma1Encoder,
    st: &mut OptimumState,
    reps: &mut [u32; REPS],
    input: &[u8],
    len_end_in: u32,
    position: u32,
    cur: u32,
    buf_avail_full: u32,
) -> u32 {
    let nice_len = st.nice_len;
    let buf = st.read_pos - 1; // decision position + cur
    let mut len_end = len_end_in;
    let matches_count = st.matches.len();
    let new_len_in = st.longest_match_length;
    let mut new_len = new_len_in;
    let mut pos_prev = st.opts[cur as usize].pos_prev as usize;
    let state_u8;

    if st.opts[cur as usize].prev_1_is_literal {
        pos_prev -= 1;
        let mut state;
        if st.opts[cur as usize].prev_2 {
            state = LzmaState::new(st.opts[st.opts[cur as usize].pos_prev_2 as usize].state);
            if st.opts[cur as usize].back_prev_2 < REPS as u32 {
                state.on_rep();
            } else {
                state.on_match();
            }
        } else {
            state = LzmaState::new(st.opts[pos_prev].state);
        }
        state.on_literal();
        state_u8 = state.as_u8();
    } else {
        state_u8 = st.opts[pos_prev].state;
    }
    let mut state = LzmaState::new(state_u8);

    if pos_prev == cur as usize - 1 {
        if is_short_rep(&st.opts[cur as usize]) {
            state.on_short_rep();
        } else {
            state.on_literal();
        }
    } else {
        let pos;
        if st.opts[cur as usize].prev_1_is_literal && st.opts[cur as usize].prev_2 {
            pos_prev = st.opts[cur as usize].pos_prev_2 as usize;
            pos = st.opts[cur as usize].back_prev_2;
            state.on_rep();
        } else {
            pos = st.opts[cur as usize].back_prev;
            if pos < REPS as u32 {
                state.on_rep();
            } else {
                state.on_match();
            }
        }

        if pos < REPS as u32 {
            reps[0] = st.opts[pos_prev].backs[pos as usize];
            for i in 1..=pos as usize {
                reps[i] = st.opts[pos_prev].backs[i - 1];
            }
            for i in (pos as usize + 1)..REPS {
                reps[i] = st.opts[pos_prev].backs[i];
            }
        } else {
            reps[0] = pos - REPS as u32;
            for i in 1..REPS {
                reps[i] = st.opts[pos_prev].backs[i - 1];
            }
        }
    }

    st.opts[cur as usize].state = state.as_u8();
    st.opts[cur as usize].backs = *reps;

    let cur_price = st.opts[cur as usize].price;
    let current_byte = input[buf];
    let match_byte = if (reps[0] as usize) < buf {
        input[buf - reps[0] as usize - 1]
    } else {
        0
    };
    let pos_state = position & enc.pb_mask;
    let state_idx = usize::from(state.as_u8());
    let pos_states = 1usize << enc.pb as usize;
    let is_match_idx = state_idx * pos_states + pos_state as usize;

    let cur_and_1_price = cur_price
        + rc_bit_0_price(enc.is_match[is_match_idx].probability())
        + get_literal_price(
            enc,
            position,
            input[buf - 1],
            !state.is_literal_context(),
            match_byte,
            current_byte,
        );

    let mut next_is_literal = false;
    if cur_and_1_price < st.opts[cur as usize + 1].price {
        st.opts[cur as usize + 1].price = cur_and_1_price;
        st.opts[cur as usize + 1].pos_prev = cur;
        make_literal(&mut st.opts[cur as usize + 1]);
        next_is_literal = true;
    }

    let match_price = cur_price + rc_bit_1_price(enc.is_match[is_match_idx].probability());
    let rep_match_price = match_price + rc_bit_1_price(enc.is_rep[state_idx].probability());

    if match_byte == current_byte
        && !(st.opts[cur as usize + 1].pos_prev < cur && st.opts[cur as usize + 1].back_prev == 0)
    {
        let short_rep_price = rep_match_price + get_short_rep_price(enc, state_idx, is_match_idx);
        if short_rep_price <= st.opts[cur as usize + 1].price {
            st.opts[cur as usize + 1].price = short_rep_price;
            st.opts[cur as usize + 1].pos_prev = cur;
            make_short_rep(&mut st.opts[cur as usize + 1]);
            next_is_literal = true;
        }
    }

    if buf_avail_full < 2 {
        return len_end;
    }

    let buf_avail = buf_avail_full.min(nice_len);

    if !next_is_literal && match_byte != current_byte {
        // Try literal + rep0.
        let back = buf - reps[0] as usize - 1;
        if (reps[0] as usize) < buf {
            let limit = buf_avail_full.min(nice_len + 1);
            let len_test = if input[buf] == input[back] {
                memcmplen(input, buf, back, 1, limit) - 1
            } else {
                0
            };
            if len_test >= 2 {
                let mut state_2 = state;
                state_2.on_literal();

                let pos_state_next = (position + 1) & enc.pb_mask;
                let state_2_idx = usize::from(state_2.as_u8());
                let state_2_is_match = state_2_idx * pos_states + pos_state_next as usize;

                let next_rep_match_price = cur_and_1_price
                    + rc_bit_1_price(enc.is_match[state_2_is_match].probability())
                    + rc_bit_1_price(enc.is_rep[state_2_idx].probability());

                let offset = cur + 1 + len_test;
                while len_end < offset {
                    len_end += 1;
                    st.opts[len_end as usize].price = RC_INFINITY_PRICE;
                }

                let cur_and_len_price = next_rep_match_price
                    + get_rep_price(enc, 0, len_test, state_2_idx, pos_state_next as usize);
                if cur_and_len_price < st.opts[offset as usize].price {
                    st.opts[offset as usize].price = cur_and_len_price;
                    st.opts[offset as usize].pos_prev = cur + 1;
                    st.opts[offset as usize].back_prev = 0;
                    st.opts[offset as usize].prev_1_is_literal = true;
                    st.opts[offset as usize].prev_2 = false;
                }
            }
        }
    }

    let mut start_len = 2u32;

    for rep_index in 0..REPS {
        let back = buf.wrapping_sub(reps[rep_index] as usize + 1);
        if reps[rep_index] as usize >= buf || not_equal_16(input, buf, back) {
            continue;
        }

        let mut len_test = memcmplen(input, buf, back, 2, buf_avail);

        while len_end < cur + len_test {
            len_end += 1;
            st.opts[len_end as usize].price = RC_INFINITY_PRICE;
        }

        let len_test_temp = len_test;
        let price =
            rep_match_price + get_pure_rep_price(enc, rep_index as u32, state_idx, is_match_idx);

        loop {
            let cur_and_len_price = price
                + enc
                    .rep_length_encoder
                    .price(len_test - MATCH_LEN_MIN, pos_state as usize);
            if cur_and_len_price < st.opts[(cur + len_test) as usize].price {
                st.opts[(cur + len_test) as usize].price = cur_and_len_price;
                st.opts[(cur + len_test) as usize].pos_prev = cur;
                st.opts[(cur + len_test) as usize].back_prev = rep_index as u32;
                st.opts[(cur + len_test) as usize].prev_1_is_literal = false;
            }
            len_test -= 1;
            if len_test < 2 {
                break;
            }
        }

        len_test = len_test_temp;
        if rep_index == 0 {
            start_len = len_test + 1;
        }

        let mut len_test_2 = len_test + 1;
        let limit = buf_avail_full.min(len_test_2 + nice_len);
        if len_test_2 < limit {
            len_test_2 = memcmplen(input, buf, back, len_test_2, limit);
        }
        len_test_2 -= len_test + 1;

        if len_test_2 >= 2 {
            let mut state_2 = state;
            state_2.on_rep();

            let pos_state_next = (position + len_test) & enc.pb_mask;
            let state_2a_idx = usize::from(state_2.as_u8());
            let state_2a_is_match = state_2a_idx * pos_states + pos_state_next as usize;

            let cur_and_len_literal_price = price
                + enc
                    .rep_length_encoder
                    .price(len_test - MATCH_LEN_MIN, pos_state as usize)
                + rc_bit_0_price(enc.is_match[state_2a_is_match].probability())
                + get_literal_price(
                    enc,
                    position + len_test,
                    input[buf + len_test as usize - 1],
                    true,
                    input[back + len_test as usize],
                    input[buf + len_test as usize],
                );

            state_2.on_literal();
            let pos_state_next2 = (position + len_test + 1) & enc.pb_mask;
            let state_2b_idx = usize::from(state_2.as_u8());
            let state_2b_is_match = state_2b_idx * pos_states + pos_state_next2 as usize;

            let next_rep_match_price = cur_and_len_literal_price
                + rc_bit_1_price(enc.is_match[state_2b_is_match].probability())
                + rc_bit_1_price(enc.is_rep[state_2b_idx].probability());

            let offset = cur + len_test + 1 + len_test_2;
            while len_end < offset {
                len_end += 1;
                st.opts[len_end as usize].price = RC_INFINITY_PRICE;
            }

            let cur_and_len_price = next_rep_match_price
                + get_rep_price(enc, 0, len_test_2, state_2b_idx, pos_state_next2 as usize);
            if cur_and_len_price < st.opts[offset as usize].price {
                st.opts[offset as usize].price = cur_and_len_price;
                st.opts[offset as usize].pos_prev = cur + len_test + 1;
                st.opts[offset as usize].back_prev = 0;
                st.opts[offset as usize].prev_1_is_literal = true;
                st.opts[offset as usize].prev_2 = true;
                st.opts[offset as usize].pos_prev_2 = cur;
                st.opts[offset as usize].back_prev_2 = rep_index as u32;
            }
        }
    }

    if new_len > buf_avail {
        new_len = buf_avail;
        let mut i = 0usize;
        while new_len > st.matches[i].len {
            i += 1;
        }
        st.matches[i].len = new_len;
        st.matches.truncate(i + 1);
    }

    if new_len >= start_len {
        let normal_match_price = match_price + rc_bit_0_price(enc.is_rep[state_idx].probability());

        while len_end < cur + new_len {
            len_end += 1;
            st.opts[len_end as usize].price = RC_INFINITY_PRICE;
        }

        let mut i = 0usize;
        while start_len > st.matches[i].len {
            i += 1;
        }

        let mut len_test = start_len;
        loop {
            let cur_back = st.matches[i].dist;
            let mut cur_and_len_price = normal_match_price
                + get_dist_len_price(enc, st, cur_back, len_test, pos_state as usize);

            if cur_and_len_price < st.opts[(cur + len_test) as usize].price {
                st.opts[(cur + len_test) as usize].price = cur_and_len_price;
                st.opts[(cur + len_test) as usize].pos_prev = cur;
                st.opts[(cur + len_test) as usize].back_prev = cur_back + REPS as u32;
                st.opts[(cur + len_test) as usize].prev_1_is_literal = false;
            }

            if len_test == st.matches[i].len {
                // Try Match + Literal + Rep0.
                let back = buf - cur_back as usize - 1;
                let mut len_test_2 = len_test + 1;
                let limit = buf_avail_full.min(len_test_2 + nice_len);
                if len_test_2 < limit {
                    len_test_2 = memcmplen(input, buf, back, len_test_2, limit);
                }
                len_test_2 -= len_test + 1;

                if len_test_2 >= 2 {
                    let mut state_2 = state;
                    state_2.on_match();

                    let pos_state_next = (position + len_test) & enc.pb_mask;
                    let state_2a_idx = usize::from(state_2.as_u8());
                    let state_2a_is_match = state_2a_idx * pos_states + pos_state_next as usize;

                    let cur_and_len_literal_price = cur_and_len_price
                        + rc_bit_0_price(enc.is_match[state_2a_is_match].probability())
                        + get_literal_price(
                            enc,
                            position + len_test,
                            input[buf + len_test as usize - 1],
                            true,
                            input[back + len_test as usize],
                            input[buf + len_test as usize],
                        );

                    state_2.on_literal();
                    let pos_state_next2 = (pos_state_next + 1) & enc.pb_mask;
                    let state_2b_idx = usize::from(state_2.as_u8());
                    let state_2b_is_match = state_2b_idx * pos_states + pos_state_next2 as usize;

                    let next_rep_match_price = cur_and_len_literal_price
                        + rc_bit_1_price(enc.is_match[state_2b_is_match].probability())
                        + rc_bit_1_price(enc.is_rep[state_2b_idx].probability());

                    let offset = cur + len_test + 1 + len_test_2;
                    while len_end < offset {
                        len_end += 1;
                        st.opts[len_end as usize].price = RC_INFINITY_PRICE;
                    }

                    cur_and_len_price = next_rep_match_price
                        + get_rep_price(enc, 0, len_test_2, state_2b_idx, pos_state_next2 as usize);

                    if cur_and_len_price < st.opts[offset as usize].price {
                        st.opts[offset as usize].price = cur_and_len_price;
                        st.opts[offset as usize].pos_prev = cur + len_test + 1;
                        st.opts[offset as usize].back_prev = 0;
                        st.opts[offset as usize].prev_1_is_literal = true;
                        st.opts[offset as usize].prev_2 = true;
                        st.opts[offset as usize].pos_prev_2 = cur;
                        st.opts[offset as usize].back_prev_2 = cur_back + REPS as u32;
                    }
                }

                i += 1;
                if i == st.matches.len() {
                    break;
                }
            }
            len_test += 1;
        }
    }

    let _ = matches_count;
    len_end
}

/// Port of `backward` — converts the DP chain into the pending-symbol
/// queue.
fn backward(st: &mut OptimumState, len_res: &mut u32, back_res: &mut u32, cur_in: u32) {
    let mut cur = cur_in as usize;
    st.opts_end_index = cur;

    let mut pos_mem = st.opts[cur].pos_prev as usize;
    let mut back_mem = st.opts[cur].back_prev;

    loop {
        if st.opts[cur].prev_1_is_literal {
            make_literal(&mut st.opts[pos_mem]);
            st.opts[pos_mem].pos_prev = (pos_mem - 1) as u32;

            if st.opts[cur].prev_2 {
                st.opts[pos_mem - 1].prev_1_is_literal = false;
                st.opts[pos_mem - 1].pos_prev = st.opts[cur].pos_prev_2;
                st.opts[pos_mem - 1].back_prev = st.opts[cur].back_prev_2;
            }
        }

        let pos_prev = pos_mem;
        let back_cur = back_mem;

        back_mem = st.opts[pos_prev].back_prev;
        pos_mem = st.opts[pos_prev].pos_prev as usize;

        st.opts[pos_prev].back_prev = back_cur;
        st.opts[pos_prev].pos_prev = cur as u32;
        cur = pos_prev;

        if cur == 0 {
            break;
        }
    }

    st.opts_current_index = st.opts[0].pos_prev as usize;
    *len_res = st.opts[0].pos_prev;
    *back_res = st.opts[0].back_prev;
}

/// Port of `lzma_mf_find` driving the BT4 finder: fills `st.matches`
/// with the improving-length ladder and sets `st.longest_match_length`.
/// Advances the finder by one position.
fn mf_find(_enc: &Lzma1Encoder, st: &mut OptimumState, input: &[u8], bt: &mut Bt4MatchFinder<'_>) {
    let pos = st.read_pos;
    st.matches.clear();

    if pos >= input.len() {
        // Nothing left to search; consume nothing.
        return;
    }

    let avail = (input.len() - pos) as u32;
    if avail < 4 {
        // header(true, 4): too little input for the 4-byte hash.
        bt.skip(pos);
        st.read_pos += 1;
        st.read_ahead += 1;
        st.longest_match_length = 0;
        return;
    }

    st.ladder.clear();
    let mut ladder = std::mem::take(&mut st.ladder);
    bt.find(pos, &mut ladder);
    st.ladder = ladder;
    st.matches
        .extend(st.ladder.iter().map(|&(len, dist)| MatchC { len, dist }));

    // The lzma_mf_find wrapper: the longest length comes from the
    // ladder's last entry (0 when empty), and a match that hit
    // nice_len gets extended up to the real cap before returning.
    let mut longest = st.matches.last().map_or(0, |m| m.len);
    if longest == st.nice_len {
        let back = pos - (st.matches.last().expect("non-empty").dist + 1) as usize;
        let limit = avail.min(MATCH_LEN_MAX);
        longest = memcmplen(input, pos, back, longest, limit);
    }
    st.longest_match_length = longest;

    st.read_pos += 1;
    st.read_ahead += 1;
}

/// Port of `mf_skip`: run `amount` positions through the finder
/// without searching.
fn mf_skip(st: &mut OptimumState, input: &[u8], bt: &mut Bt4MatchFinder<'_>, amount: u32) {
    for _ in 0..amount {
        let p = st.read_pos;
        if p + 4 <= input.len() {
            bt.skip(p);
        }
        st.read_pos += 1;
    }
    st.read_ahead += amount;
}

// ---------- price helpers ----------

fn get_literal_price(
    enc: &Lzma1Encoder,
    position: u32,
    prev_byte: u8,
    match_mode: bool,
    match_byte: u8,
    symbol: u8,
) -> u32 {
    let lit_state = (position << 8 | u32::from(prev_byte)) & enc.literal_mask;
    let models = enc.literal_encoder.models();
    let base = (3 * (lit_state << enc.lc)) as usize;

    if match_mode {
        // Mirror LiteralEncoder::encode_matched's walk.
        let mut price = 0u32;
        let mut symbol = u32::from(symbol);
        symbol += 1 << 8;
        let mut offset = 0x100u32;
        let mut match_byte = u32::from(match_byte);
        loop {
            match_byte <<= 1;
            let match_bit = match_byte & offset;
            let subcoder_index = offset + match_bit + (symbol >> 8);
            let bit = (symbol >> 7) & 1;
            price += rc_bit_price(models[base + subcoder_index as usize].probability(), bit);
            symbol <<= 1;
            offset &= !(match_byte ^ symbol);
            if symbol >= 1 << 16 {
                break;
            }
        }
        price
    } else {
        rc_bittree_price(&models[base..base + 0x100], 8, u32::from(symbol))
    }
}

fn get_short_rep_price(enc: &Lzma1Encoder, state_idx: usize, is_match_idx: usize) -> u32 {
    rc_bit_0_price(enc.is_rep0[state_idx].probability())
        + rc_bit_0_price(enc.is_rep0_long[is_match_idx].probability())
}

fn get_pure_rep_price(
    enc: &Lzma1Encoder,
    rep_index: u32,
    state_idx: usize,
    is_long_idx: usize,
) -> u32 {
    if rep_index == 0 {
        rc_bit_0_price(enc.is_rep0[state_idx].probability())
            + rc_bit_1_price(enc.is_rep0_long[is_long_idx].probability())
    } else if rep_index == 1 {
        rc_bit_1_price(enc.is_rep0[state_idx].probability())
            + rc_bit_0_price(enc.is_rep1[state_idx].probability())
    } else {
        rc_bit_1_price(enc.is_rep0[state_idx].probability())
            + rc_bit_1_price(enc.is_rep1[state_idx].probability())
            + rc_bit_price(enc.is_rep2[state_idx].probability(), rep_index - 2)
    }
}

fn get_rep_price(
    enc: &Lzma1Encoder,
    rep_index: u32,
    len: u32,
    state_idx: usize,
    pos_state: usize,
) -> u32 {
    enc.rep_length_encoder.price(len - MATCH_LEN_MIN, pos_state)
        + get_pure_rep_price(enc, rep_index, state_idx, pos_state)
}

fn get_dist_len_price(
    enc: &Lzma1Encoder,
    st: &OptimumState,
    dist: u32,
    len: u32,
    pos_state: usize,
) -> u32 {
    let len_code = len - MATCH_LEN_MIN;
    let dist_state = (len_code.min(4 - 1)) as usize;
    let price = if (dist as usize) < FULL_DISTANCES {
        st.dist_prices[dist_state][dist as usize]
    } else {
        let slot = distance_slot(dist);
        st.dist_slot_prices[dist_state][slot as usize]
            + st.align_prices[(dist & ALIGN_MASK) as usize]
    };
    price + enc.length_encoder.price(len_code, pos_state)
}

impl Lzma1Encoder {
    /// Port of `fill_dist_prices`.
    pub(crate) fn optimum_fill_dist_prices(&self, st: &mut OptimumState) {
        for dist_state in 0..4usize {
            let slots = self.distance_encoder.slot_models(dist_state);
            for slot in 0..st.dist_table_size {
                st.dist_slot_prices[dist_state][slot] = rc_bittree_price(slots, 6, slot as u32);
            }
            for slot in DIST_MODEL_END as usize..st.dist_table_size {
                st.dist_slot_prices[dist_state][slot] +=
                    rc_direct_price((((slot >> 1) - 1) as u32).saturating_sub(4));
            }
            for i in 0..DIST_MODEL_START as usize {
                st.dist_prices[dist_state][i] = st.dist_slot_prices[dist_state][i];
            }
        }

        let special = self.distance_encoder.special_models();
        for i in DIST_MODEL_START as usize..FULL_DISTANCES {
            let slot = distance_slot(i as u32);
            let footer_bits = (slot >> 1) - 1;
            let base = (2 | (slot & 1)) << footer_bits;
            // C passes `dist_special + base - dist_slot - 1`, which is
            // -1 for slot 4 — legal there because the reverse tree
            // starts at model index 1. Index with an isize base.
            let start = base as isize - slot as isize - 1;
            let mut price = 0u32;
            let mut model_index = 1u32;
            let mut symbol = i as u32 - base;
            let mut remaining = footer_bits;
            loop {
                let bit = symbol & 1;
                symbol >>= 1;
                price += rc_bit_price(
                    special[(start + model_index as isize) as usize].probability(),
                    bit,
                );
                model_index = (model_index << 1) + bit;
                remaining -= 1;
                if remaining == 0 {
                    break;
                }
            }
            for dist_state in 0..4usize {
                st.dist_prices[dist_state][i] =
                    price + st.dist_slot_prices[dist_state][slot as usize];
            }
        }

        st.match_price_count = 0;
    }

    /// Port of `fill_align_prices`.
    pub(crate) fn optimum_fill_align_prices(&self, st: &mut OptimumState) {
        let align = self.distance_encoder.align_models();
        for i in 0..ALIGN_SIZE {
            st.align_prices[i] = rc_bittree_reverse_price(align, 4, i as u32);
        }
        st.align_price_count = 0;
    }

    /// Bytes the range encoder has produced so far this chunk.
    pub(crate) fn range_encoder_bytes(&self) -> usize {
        self.range_encoder.bytes_for_decode()
    }

    /// Finish the current chunk's range coding: flush, take the
    /// output, and start a fresh range encoder (probability models
    /// carry — the LZMA2 reset level 0 continuation).
    pub(crate) fn take_range_encoder(&mut self) -> Vec<u8> {
        self.range_encoder.flush();
        let mut next = crate::range_coder::RangeEncoder::new();
        next.set_pad_flush();
        std::mem::replace(&mut self.range_encoder, next).finish()
    }

    /// Port of `lzma_lzma_optimum_normal` — parse the next symbol.
    /// `position` is the absolute uncompressed offset of the decision
    /// point. Returns `(back, len)`; `back == u32::MAX` means literal,
    /// `back < 4` a rep index, otherwise the 0-based match distance is
    /// `back - 4`.
    pub(crate) fn optimum_next_symbol(
        &mut self,
        st: &mut OptimumState,
        input: &[u8],
        bt: &mut Bt4MatchFinder<'_>,
        position: u32,
    ) -> (u32, u32) {
        if st.opts_end_index != st.opts_current_index {
            let len_res = st.opts[st.opts_current_index].pos_prev - st.opts_current_index as u32;
            let back_res = st.opts[st.opts_current_index].back_prev;
            st.opts_current_index = st.opts[st.opts_current_index].pos_prev as usize;
            return (back_res, len_res);
        }

        if st.read_ahead == 0 {
            if st.match_price_count >= (1 << 7) {
                self.optimum_fill_dist_prices(st);
            }
            if st.align_price_count >= ALIGN_SIZE as u32 {
                self.optimum_fill_align_prices(st);
            }
        }

        let mut back_res = 0u32;
        let mut len_res = 0u32;
        let len_end = helper1(self, st, input, bt, position, &mut back_res, &mut len_res);
        let Some(mut len_end) = len_end else {
            return (back_res, len_res);
        };

        let mut reps: [u32; REPS] = [self.rep0, self.rep1, self.rep2, self.rep3];

        let mut cur = 1u32;
        while cur < len_end {
            mf_find(self, st, input, bt);
            let longest = st.longest_match_length;
            if longest >= st.nice_len {
                break;
            }

            let avail_full = (input.len() - (st.read_pos - 1)) as u32;
            let buf_avail_full = avail_full.min(OPTS as u32 - 1 - cur);
            len_end = helper2(
                self,
                st,
                &mut reps,
                input,
                len_end,
                position + cur,
                cur,
                buf_avail_full,
            );

            cur += 1;
        }

        backward(st, &mut len_res, &mut back_res, cur);
        (back_res, len_res)
    }

    /// Port of `encode_init` — the very first byte of the stream must
    /// be encoded as a literal (a rep/shortrep at position 0 would be
    /// an invalid distance).
    pub(crate) fn optimum_init(
        &mut self,
        st: &mut OptimumState,
        input: &[u8],
        bt: &mut Bt4MatchFinder<'_>,
    ) {
        if input.is_empty() {
            return;
        }
        mf_skip(st, input, bt, 1);
        st.read_ahead = 0;
        self.encode_literal_byte(input[0], 0, 0, 0);
    }

    /// Port of `encode_symbol` — emit one parsed symbol at absolute
    /// position `pos`, updating price-refresh counters.
    pub(crate) fn optimum_emit_symbol(
        &mut self,
        input: &[u8],
        back: u32,
        len: u32,
        pos: usize,
        st: &mut OptimumState,
    ) {
        let abs_pos = self.base_pos.wrapping_add(pos as u32);
        let pos_state = (abs_pos & self.pb_mask) as usize;

        if back == u32::MAX {
            let prev_byte = if pos > 0 {
                input[pos - 1]
            } else {
                self.base_prev_byte
            };
            let match_byte = self.get_match_byte(input, pos);
            self.encode_literal_byte(input[pos], prev_byte, match_byte, pos);
        } else if back < REPS as u32 {
            if back == 0 && len == 1 {
                self.encode_short_rep(pos);
            } else {
                self.encode_rep_match(back, len, pos);
                self.rep_length_encoder.note_encoded(pos_state);
            }
        } else {
            let dist_1based = back - REPS as u32 + 1;
            let slot = distance_slot(back - REPS as u32);
            self.encode_match(dist_1based, len, pos);
            self.length_encoder.note_encoded(pos_state);
            st.match_price_count = st.match_price_count.wrapping_add(1);
            if slot >= DIST_MODEL_END {
                st.align_price_count = st.align_price_count.wrapping_add(1);
            }
        }

        st.read_ahead = st.read_ahead.saturating_sub(len);
        if st.stats_file_pos.is_some() {
            use std::cell::RefCell;
            thread_local! {
                static STATS: RefCell<[u64; 8]> = const { RefCell::new([0; 8]) };
            }
            STATS.with(|s| {
                let mut s = s.borrow_mut();
                if back == u32::MAX {
                    s[0] += 1;
                } else if back < REPS as u32 {
                    if len == 1 {
                        s[1] += 1;
                    } else {
                        s[2] += 1;
                    }
                } else {
                    s[3] += 1;
                    s[4] += u64::from(len);
                    s[5] += u64::from(back - REPS as u32);
                }
                s[6] += u64::from(len);
                if let Some(f) = st.stats_file_pos {
                    if s[6] >= f && s[7] == 0 {
                        s[7] = 1;
                        eprintln!(
                            "symstats: literal={} shortrep={} rep={} match={} matchlen={} distsum={}",
                            s[0], s[1], s[2], s[3], s[4], s[5]
                        );
                    }
                }
            });
        }
    }
}
