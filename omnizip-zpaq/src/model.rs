//! Order-2 context model with adaptive bit probabilities.
//!
//! The model maintains a probability estimate for the next bit, conditioned
//! on the last two bits observed in the current order-2 context (the two
//! most recently encoded bits). Each context has a pair of frequency
//! counters `(n0, n1)` updated after every coded bit; the probability
//! `P(bit=1)` returned to the arithmetic coder is
//! `(n1 + 1) / (n0 + n1 + 2)`, mapped to a `u16` in `[1, 65535]`.
//!
//! ## Why order-2 over the *bit* stream?
//!
//! A bit-level order-2 context adapts quickly and is cheap (only 16 entries
//! are needed). To get useful redundancy from a byte-level model we also fold
//! in the previous byte value as a second dimension: the context key is the
//! concatenation of the last two byte values (16 bits, 65 536 entries) plus
//! the current bit position within the byte. This is the canonical "order-2"
//! bit-context model used by simple PAQ-style coders.
//!
//! ## Storage
//!
//! A dense `[[Counter; 256]; 65536]` table would consume 32 MB. Instead we
//! use a `HashMap<(u16, u8), Counter>` so only contexts that actually occur
//! are stored — typically a few thousand entries for text inputs.

#![forbid(unsafe_code)]
// Probability/counter arithmetic involves narrowing casts that are
// provably safe (values are bounded by MAX_COUNT, PROB_SCALE, etc.) but
// flagged by clippy::pedantic.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::collections::HashMap;

use crate::arithmetic::{ArithmeticDecoder, ArithmeticEncoder, PROB_SCALE};

/// Maximum count value before both counters are halved (preventing overflow
/// and providing gradual forgetting of old statistics).
const MAX_COUNT: u16 = 1 << 14;

/// A pair of bit-frequency counters.
#[derive(Clone, Copy, Debug, Default)]
struct Counter {
    n0: u16,
    n1: u16,
}

impl Counter {
    /// Probability that the next bit is 1, as a `u16` in `[1, 65535]`.
    fn prob_one(self) -> u16 {
        let denom = u64::from(self.n0) + u64::from(self.n1) + 2;
        let num = u64::from(self.n1) + 1;
        // Result fits in u64; map to [1, PROB_SCALE-1].
        let scaled = (num * (PROB_SCALE - 1)) / denom + 1;
        let cap = PROB_SCALE - 1;
        u16::try_from(scaled.min(cap)).unwrap_or(u16::MAX)
    }

    fn observe(&mut self, bit: bool) {
        if bit {
            self.n1 = self.n1.saturating_add(1);
        } else {
            self.n0 = self.n0.saturating_add(1);
        }
        if self.n0 >= MAX_COUNT || self.n1 >= MAX_COUNT {
            self.n0 = self.n0 / 2 + (self.n0 & 1);
            self.n1 = self.n1 / 2 + (self.n1 & 1);
        }
    }
}

/// Order-2 byte-context adaptive model. Stateless across encode/decode —
/// the model is rebuilt identically on both sides because it depends only
/// on already-decoded data.
pub struct Order2Model {
    table: HashMap<(u16, u8), Counter>,
    last_byte: u8,
    prev_byte: u8,
}

impl Default for Order2Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Order2Model {
    #[must_use]
    pub fn new() -> Self {
        Self {
            table: HashMap::new(),
            last_byte: 0,
            prev_byte: 0,
        }
    }

    /// Context key = (`prev_byte` || `last_byte`) packed as u16 + bit position.
    fn key(&self, bit_pos: u8) -> (u16, u8) {
        let ctx = (u16::from(self.prev_byte) << 8) | u16::from(self.last_byte);
        (ctx, bit_pos)
    }

    fn prob(&self, bit_pos: u8) -> u16 {
        match self.table.get(&self.key(bit_pos)) {
            Some(c) => c.prob_one(),
            None => 1 << 15,
        }
    }

    fn update(&mut self, bit_pos: u8, bit: bool) {
        let key = self.key(bit_pos);
        let entry = self.table.entry(key).or_default();
        entry.observe(bit);
    }

    fn advance_byte(&mut self, byte: u8) {
        self.prev_byte = self.last_byte;
        self.last_byte = byte;
    }

    /// Look up the per-context probability using an *explicit* byte context,
    /// rather than the model's internal `(prev_byte, last_byte)` state.
    /// Used by [`MultiModel`] which manages its own byte context.
    pub(crate) fn prob_with_context(&self, prev_byte: u8, last_byte: u8, bit_pos: u8) -> u16 {
        let ctx = (u16::from(prev_byte) << 8) | u16::from(last_byte);
        match self.table.get(&(ctx, bit_pos)) {
            Some(c) => c.prob_one(),
            None => 1 << 15,
        }
    }

    /// Update the per-context counter using an *explicit* byte context.
    pub(crate) fn update_with_context(
        &mut self,
        prev_byte: u8,
        last_byte: u8,
        bit_pos: u8,
        bit: bool,
    ) {
        let ctx = (u16::from(prev_byte) << 8) | u16::from(last_byte);
        let entry = self.table.entry((ctx, bit_pos)).or_default();
        entry.observe(bit);
    }

    /// Encode a byte MSB-first using the current model.
    pub fn encode_byte(&mut self, byte: u8, enc: &mut ArithmeticEncoder) {
        for bit_pos in 0..8 {
            let bit = (byte >> (7 - bit_pos)) & 1 == 1;
            let prob = self.prob(bit_pos);
            enc.encode_bit(prob, bit);
            self.update(bit_pos, bit);
        }
        self.advance_byte(byte);
    }

    /// Decode a byte MSB-first using the current model.
    pub fn decode_byte(&mut self, dec: &mut ArithmeticDecoder) -> u8 {
        let mut byte: u8 = 0;
        for bit_pos in 0..8 {
            let prob = self.prob(bit_pos);
            let bit = dec.decode_bit(prob);
            self.update(bit_pos, bit);
            if bit {
                byte |= 1 << (7 - bit_pos);
            }
        }
        self.advance_byte(byte);
        byte
    }
}

// ---------------------------------------------------------------------------
// Phase 2: multi-model context mixing.
// ---------------------------------------------------------------------------

/// Number of models feeding the [`Mixer`](crate::mixer::Mixer).
///
/// Must agree with [`crate::mixer::NUM_MODELS`]. Seven models:
/// order-0, order-1, order-2, order-3, match, run-length, word.
pub const NUM_MODELS: usize = 7;

/// Maximum count before a counter pair is halved. Same value as the
/// order-2 model uses, kept consistent so all models forget at the same rate.
const MAX_COUNT_P2: u16 = 1 << 14;

/// Probability returned by a model that has seen no observations yet.
/// Represents maximum uncertainty (50/50).
const DEFAULT_PROB: u16 = 1 << 15;

/// A reusable frequency-counter pair shared across the order-N models.
#[derive(Clone, Copy, Debug, Default)]
struct CounterPair {
    n0: u16,
    n1: u16,
}

impl CounterPair {
    fn prob_one(self) -> u16 {
        let denom = u64::from(self.n0) + u64::from(self.n1) + 2;
        let num = u64::from(self.n1) + 1;
        let scaled = (num * (PROB_SCALE - 1)) / denom + 1;
        let cap = PROB_SCALE - 1;
        scaled.min(cap) as u16
    }

    fn observe(&mut self, bit: bool) {
        if bit {
            self.n1 = self.n1.saturating_add(1);
        } else {
            self.n0 = self.n0.saturating_add(1);
        }
        if self.n0 >= MAX_COUNT_P2 || self.n1 >= MAX_COUNT_P2 {
            self.n0 = self.n0 / 2 + (self.n0 & 1);
            self.n1 = self.n1 / 2 + (self.n1 & 1);
        }
    }
}

/// Order-0 model: tracks bit frequencies globally (no context).
#[derive(Debug, Default)]
pub struct Order0Model {
    /// One counter pair per bit-position within the byte (0..8).
    /// Bit position gives a tiny amount of structure (e.g. MSB tends to be
    /// the sign/leading bit) without carrying any byte-context information.
    counters: [CounterPair; 8],
}

impl Order0Model {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn prob(bit_pos: u8, counters: &[CounterPair; 8]) -> u16 {
        counters[usize::from(bit_pos)].prob_one()
    }

    #[inline]
    fn update(&mut self, bit_pos: u8, bit: bool) {
        self.counters[usize::from(bit_pos)].observe(bit);
    }
}

/// Order-1 model: context = previous byte value (256 buckets) + bit position.
#[derive(Debug)]
pub struct Order1Model {
    /// 256 byte contexts * 8 bit positions = 2048 counter pairs.
    /// Dense storage is cheap (8 KB) and avoids `HashMap` overhead.
    counters: [[CounterPair; 8]; 256],
}

impl Default for Order1Model {
    fn default() -> Self {
        // Arrays larger than 32 elements do not derive `Default`, so we
        // build it explicitly with `[[Default::default(); 8]; 256]` via
        // a const-evaluated helper.
        Self {
            counters: [[CounterPair::default(); 8]; 256],
        }
    }
}

impl Order1Model {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn prob(prev_byte: u8, bit_pos: u8, counters: &[[CounterPair; 8]; 256]) -> u16 {
        counters[usize::from(prev_byte)][usize::from(bit_pos)].prob_one()
    }

    #[inline]
    fn update(&mut self, prev_byte: u8, bit_pos: u8, bit: bool) {
        self.counters[usize::from(prev_byte)][usize::from(bit_pos)].observe(bit);
    }
}

/// Match model: when the most-recent bytes occur earlier in the history,
/// predict the next bit by replaying the matching byte.
///
/// Concretely, the model keeps a sliding window over recent history (a ring
/// buffer of the last [`MATCH_HISTORY`] bytes). On each prediction, it
/// searches the recent history for the longest suffix of the current
/// `(prev_byte, last_byte)` pair; if found, the predicted probability is
/// near-certain (high) for the bits of the byte that followed the match.
#[derive(Debug)]
pub struct MatchModel {
    /// Ring buffer of recent bytes (oldest first within the buffer).
    history: Vec<u8>,
    /// Current length of the active match, or 0 if no match is in progress.
    match_len: u32,
    /// Byte position (within `history`) where the active match started + 1.
    match_pos: usize,
    /// Bit position within the predicted byte that we are currently emitting
    /// (0..8). When 8, the match is exhausted and a new search is triggered
    /// on the next prediction.
    bit_pos: u8,
    /// The predicted next byte (set when a match is active).
    predicted_byte: u8,
}

/// Number of recent bytes retained for match searches. Larger windows find
/// longer matches at higher memory cost; 4 KB is a reasonable default for
/// short-to-medium text inputs.
const MATCH_HISTORY: usize = 4096;

impl Default for MatchModel {
    fn default() -> Self {
        Self::new()
    }
}

impl MatchModel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            history: Vec::with_capacity(MATCH_HISTORY),
            match_len: 0,
            match_pos: 0,
            bit_pos: 0,
            predicted_byte: 0,
        }
    }

    /// Append a byte to the rolling history, evicting the oldest if full.
    fn push_byte(&mut self, byte: u8) {
        if self.history.len() >= MATCH_HISTORY {
            self.history.remove(0);
        }
        self.history.push(byte);
    }

    /// Search the history for the longest occurrence of the 2-byte key
    /// `(prev_byte, last_byte)`. Returns the index of the match (the byte
    /// *after* which is the prediction source), or `None` if not found.
    ///
    /// We search from the end of the buffer toward the start so the most
    /// recent (and most likely to still match) occurrence wins.
    fn find_match(&self, prev_byte: u8, last_byte: u8) -> Option<usize> {
        // Walk backward so the latest occurrence is returned first.
        let n = self.history.len();
        if n < 3 {
            return None;
        }
        // Compare pairs (history[i], history[i+1]) for i in [0, n-2).
        let mut i = n - 2;
        loop {
            if self.history[i] == prev_byte && self.history[i + 1] == last_byte {
                // Return the index of the byte *after* the pair, if any.
                if i + 2 < n {
                    return Some(i + 2);
                }
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
        None
    }

    /// Return the predicted probability for the given bit position.
    ///
    /// Returns [`DEFAULT_PROB`] when no match is active (50/50), or a
    /// strongly-biased probability (set by [`MATCH_CONFIDENCE`]) when a match
    /// is in progress.
    fn prob(&self) -> u16 {
        if self.match_len > 0 && self.bit_pos < 8 {
            // The predicted bit.
            let predicted_bit = (self.predicted_byte >> (7 - self.bit_pos)) & 1 == 1;
            // The complementary probability lives in the u16 range; compute
            // it via unsigned arithmetic to avoid underflow.
            if predicted_bit {
                MATCH_CONFIDENCE
            } else {
                u16::MAX - MATCH_CONFIDENCE + 1
            }
        } else {
            DEFAULT_PROB
        }
    }

    /// Called after each bit is coded. Advances the bit position; when the
    /// byte is complete (`bit_pos == 8`), the next prediction will trigger
    /// a fresh match search via [`Self::begin_byte`].
    fn update(&mut self) {
        self.bit_pos = self.bit_pos.saturating_add(1);
    }

    /// Begin a new byte: search for a match and, if found, latch onto the
    /// predicted next byte.
    fn begin_byte(&mut self, prev_byte: u8, last_byte: u8) {
        self.bit_pos = 0;
        if let Some(pos) = self.find_match(prev_byte, last_byte) {
            self.match_pos = pos;
            self.match_len = 1;
            self.predicted_byte = self.history.get(pos).copied().unwrap_or(0);
        } else {
            self.match_len = 0;
        }
    }

    /// Finalise the byte after all 8 bits have been coded: push the byte
    /// into the history.
    fn end_byte(&mut self, byte: u8) {
        self.push_byte(byte);
    }
}

/// Confidence (as a probability that the predicted bit is correct) emitted
/// by [`MatchModel`] when a match is in progress. Kept moderate so that a
/// wrong match doesn't produce an extreme residual that the arithmetic
/// coder pays heavily for; the mixer's adaptation handles weighting.
const MATCH_CONFIDENCE: u16 = 52_000; // ~0.79

// ---------------------------------------------------------------------------
// Run-length model.
// ---------------------------------------------------------------------------

/// Run-length model: predicts that the next byte will equal the previous
/// byte when the recent history is a run of identical bytes.
///
/// After two or more consecutive identical bytes, this model emits a
/// strongly-biased probability toward the run byte's bits. Below that
/// threshold it emits the default 50/50 probability. The mixer learns
/// when to trust this signal (highly repetitive data) vs. ignore it
/// (mixed text/binary).
///
/// Unlike [`Order2Model`] / [`MatchModel`], this model has no per-context
/// table — its state is just `(last_byte, run_length, current_bit_pos)`.
/// That makes it cheap and a useful sharp signal in the mix.
#[derive(Debug)]
pub struct RunLengthModel {
    last_byte: u8,
    run_length: u32,
    bit_pos: u8,
}

impl Default for RunLengthModel {
    fn default() -> Self {
        Self::new()
    }
}

impl RunLengthModel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_byte: 0,
            run_length: 0,
            bit_pos: 0,
        }
    }

    /// Begin a new byte: reset the per-byte bit position.
    fn begin_byte(&mut self) {
        self.bit_pos = 0;
    }

    /// Predict the next bit. When in a run of ≥ 2 identical bytes, predict
    /// the run-byte's bit strongly. Otherwise return default uncertainty.
    fn prob(&self) -> u16 {
        if self.run_length >= 2 && self.bit_pos < 8 {
            let predicted_bit = (self.last_byte >> (7 - self.bit_pos)) & 1 == 1;
            if predicted_bit {
                RUN_CONFIDENCE
            } else {
                u16::MAX - RUN_CONFIDENCE + 1
            }
        } else {
            DEFAULT_PROB
        }
    }

    /// Advance the bit position after each coded bit.
    fn update(&mut self) {
        self.bit_pos = self.bit_pos.saturating_add(1);
    }

    /// Finalise the byte: update the run counter based on whether the new
    /// byte extends or breaks the current run.
    fn end_byte(&mut self, byte: u8) {
        if byte == self.last_byte {
            self.run_length = self.run_length.saturating_add(1);
        } else {
            self.run_length = 1;
            self.last_byte = byte;
        }
    }
}

/// Confidence for [`RunLengthModel`] when in a run. Slightly higher than
/// [`MATCH_CONFIDENCE`] because a run is a stronger signal than a single
/// match (the run is *current*, the match is recalled).
const RUN_CONFIDENCE: u16 = 54_000; // ~0.82

// ---------------------------------------------------------------------------
// Word-level model.
// ---------------------------------------------------------------------------

/// Word-level model: tokenises the input stream into ASCII alphanumeric
/// runs and predicts the next byte from the current word's prefix hash.
///
/// The model is **gated** by a warmup counter — before
/// [`WORD_WARMUP_BYTES`] bytes have been processed it returns the
/// neutral 50/50 probability so the mixer's `stretch()` contributes
/// nothing. After warmup the model emits a biased probability based
/// on the frequency table. This avoids regressing on short inputs
/// where adaptation hasn't converged.
///
/// Memory is bounded: both hashmaps evict arbitrary entries when they
/// exceed [`WORD_TABLE_CAP`].
///
/// ## Performance
///
/// Probabilities are precomputed per-bit-position (0..8) when the
/// word hash changes, then cached. `prob(bit_pos)` is O(1).
///
/// The expensive operation is `refresh_cache`, called whenever the
/// current word hash changes or its frequency table is updated.
/// Without incremental aggregation this would be O(N) per refresh
/// (where N is the total number of `(hash, byte)` entries — up to
/// `65_536`). To make it O(8) instead, we maintain a parallel
/// `bit_aggregate` table that tracks `(n0, n1)` counts per bit
/// position **per hash**. Updates cost O(8) on every insertion;
/// refresh costs O(8) per call.
pub struct WordModel {
    /// Frequency of (current word prefix hash, byte) → count.
    /// Key = (`word_hash`, `next_byte`).
    next_byte_freq: HashMap<(u32, u8), u16>,
    /// Per-hash, per-bit-position aggregate counts.
    /// `bit_aggregate[h]` = 8 `(n0, n1)` pairs summarising all
    /// `(h, byte)` frequencies for hash `h`. Maintained incrementally
    /// on every `next_byte_freq` update.
    bit_aggregate: HashMap<u32, [(u64, u64); 8]>,
    /// Frequency of (current word prefix hash) → count.
    word_freq: HashMap<u32, u16>,
    /// Hash of the current word's accumulated bytes (FNV-1a).
    current_word_hash: u32,
    /// Bytes accumulated in the current word so far.
    current_word_len: u8,
    /// Are we currently inside a word (alphanumeric run)?
    in_word: bool,
    /// Total bytes processed (for warmup gate).
    bytes_processed: u64,
    /// Cached probabilities for the current word hash, one per bit
    /// position (0..8). Recomputed when the hash changes or when
    /// `next_byte_freq` is updated.
    cached_probs: [u16; 8],
    /// True if `cached_probs` reflects the current `current_word_hash`.
    cache_valid: bool,
}

/// Warmup: number of bytes to process before the `WordModel` starts
/// emitting non-uniform probabilities. Picked empirically — small
/// enough that the model engages within a typical text paragraph,
/// large enough that the mixer has converged on the dominant
/// order-1/order-2 signal first.
const WORD_WARMUP_BYTES: u64 = 16_384;

/// Maximum entries in each frequency table. Beyond this, the table
/// is cleared (a "flush" — adaptation restarts). This bounds memory
/// at ~16 MiB worst case (2 × `65_536` × ~64 B/entry).
const WORD_TABLE_CAP: usize = 65_536;

impl Default for WordModel {
    fn default() -> Self {
        Self::new()
    }
}

impl WordModel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_byte_freq: HashMap::new(),
            bit_aggregate: HashMap::new(),
            word_freq: HashMap::new(),
            current_word_hash: 0x811C_9DC5, // FNV-1a offset basis
            current_word_len: 0,
            in_word: false,
            bytes_processed: 0,
            cached_probs: [DEFAULT_PROB; 8],
            cache_valid: false,
        }
    }

    /// Begin a new byte: update word-state from the previous byte.
    /// Called once per byte before the bit loop.
    fn begin_byte(&mut self, prev_byte: u8) {
        // The actual word-boundary logic runs in `end_byte` so we know
        // the byte that just got coded.
        let _ = prev_byte;
    }

    /// Probability for the current bit. Returns uniform during warmup
    /// or when outside a word.
    ///
    /// O(1) — looks up a cached per-bit-position probability that was
    /// precomputed when the current word hash was finalised.
    fn prob(&self, bit_pos: u8) -> u16 {
        if self.bytes_processed < WORD_WARMUP_BYTES || !self.in_word || self.current_word_len < 3 {
            return DEFAULT_PROB;
        }
        if !self.cache_valid {
            // Cache is stale; caller should have refreshed it. Return
            // uniform as a safe fallback.
            return DEFAULT_PROB;
        }
        self.cached_probs[usize::from(bit_pos)]
    }

    /// Recompute the 8 per-bit-position probabilities from
    /// `bit_aggregate` for the current word hash. Called when the
    /// hash changes or when a frequency entry is added/updated.
    ///
    /// O(8): looks up the per-hash aggregate (maintained incrementally
    /// on every `next_byte_freq` update) and folds it into the
    /// Laplace-smoothed probability for each bit position.
    fn refresh_cache(&mut self) {
        if !self.in_word || self.current_word_len < 3 {
            self.cached_probs = [DEFAULT_PROB; 8];
            self.cache_valid = true;
            return;
        }
        let hash = self.current_word_hash;
        match self.bit_aggregate.get(&hash) {
            None => {
                self.cached_probs = [DEFAULT_PROB; 8];
            }
            Some(agg) => {
                for bit_pos in 0..8usize {
                    let (n0, n1) = agg[bit_pos];
                    let total = n0 + n1 + 2;
                    let p1 = (((n1 + 1) * (PROB_SCALE - 1)) / total + 1).min(PROB_SCALE - 1);
                    self.cached_probs[bit_pos] = p1 as u16;
                }
            }
        }
        self.cache_valid = true;
    }

    /// Update `bit_aggregate` for the given `(hash, byte)` frequency
    /// delta. O(8): touches one entry per bit position.
    fn update_bit_aggregate(&mut self, hash: u32, byte: u8, delta: u64) {
        let agg = self
            .bit_aggregate
            .entry(hash)
            .or_insert_with(|| [(0u64, 0u64); 8]);
        for bit_pos in 0..8usize {
            if (u16::from(byte) >> (7 - bit_pos)) & 1 == 1 {
                agg[bit_pos].1 = agg[bit_pos].1.saturating_add(delta);
            } else {
                agg[bit_pos].0 = agg[bit_pos].0.saturating_add(delta);
            }
        }
    }

    /// Advance the bit position after each coded bit. No-op now that
    /// `prob(bit_pos)` takes the position as an argument and reads
    /// from a precomputed cache.
    fn update(&mut self) {
        // Intentionally empty: the cache holds all 8 bit positions
        // simultaneously, so there's no per-bit state to advance.
    }

    /// Finalise the byte: update word state and frequency tables.
    fn end_byte(&mut self, byte: u8) {
        self.bytes_processed = self.bytes_processed.saturating_add(1);
        let is_alnum = byte.is_ascii_alphanumeric() || byte == b'_';

        let mut hash_changed = false;
        if is_alnum {
            // Extend the current word hash.
            self.current_word_hash ^= u32::from(byte);
            self.current_word_hash = self.current_word_hash.wrapping_mul(0x0100_0193);
            self.current_word_len = self.current_word_len.saturating_add(1);
            self.in_word = true;
            hash_changed = true;
        } else if self.in_word {
            // Word just ended. Record its frequency.
            self.flush_word_if_needed();
            *self.word_freq.entry(self.current_word_hash).or_insert(0) += 1;
            // Reset for next word.
            self.current_word_hash = 0x811C_9DC5;
            self.current_word_len = 0;
            self.in_word = false;
            hash_changed = true;
        }

        // Update next-byte frequency: given current word prefix hash,
        // record that this byte followed it.
        if self.in_word && self.current_word_len >= 3 {
            let cap_ok = self.next_byte_freq.len() < WORD_TABLE_CAP;
            if cap_ok {
                let key = (self.current_word_hash, byte);
                let entry = self.next_byte_freq.entry(key).or_insert(0);
                *entry += 1;
                // Maintain the parallel bit-aggregate so refresh_cache
                // is O(8) instead of O(N).
                self.update_bit_aggregate(self.current_word_hash, byte, 1);
                self.cache_valid = false;
            } else {
                // Cap exceeded — flush all tables.
                self.next_byte_freq.clear();
                self.bit_aggregate.clear();
                self.word_freq.clear();
                self.cache_valid = false;
            }
        } else if hash_changed {
            self.cache_valid = false;
        }

        // Refresh the cache for the next byte's bit loop. After warmup
        // this is the only per-byte O(N_active_entries) work; the
        // per-bit `prob()` calls are then O(1).
        if !self.cache_valid {
            self.refresh_cache();
        }
    }

    /// Reset the word hash if we've crossed the cap during accumulation.
    fn flush_word_if_needed(&mut self) {
        if self.word_freq.len() >= WORD_TABLE_CAP {
            self.word_freq.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// Order-3 model.
// ---------------------------------------------------------------------------

/// Order-3 model: context = three previous bytes packed into a `u32` +
/// bit position. Sparse storage via `HashMap` — only contexts that
/// actually occur are stored.
///
/// Order-3 catches longer-range dependencies than order-2 (e.g.
/// distinguishing "the " from "the" + 'm'). Diminishing returns
/// beyond order-3 for byte-level prediction on text-sized inputs,
/// but useful in the mix when the corpus has repetitive triples.
pub struct Order3Model {
    table: HashMap<(u32, u8), CounterPair>,
}

impl Default for Order3Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Order3Model {
    #[must_use]
    pub fn new() -> Self {
        Self {
            table: HashMap::new(),
        }
    }

    /// Probability lookup with an explicit 3-byte context (used by
    /// [`MultiModel`] which manages its own byte history).
    fn prob_with_context(&self, prev2: u8, prev1: u8, last: u8, bit_pos: u8) -> u16 {
        let ctx = make_order3_ctx(prev2, prev1, last);
        match self.table.get(&(ctx, bit_pos)) {
            Some(c) => c.prob_one(),
            None => DEFAULT_PROB,
        }
    }

    fn update_with_context(&mut self, prev2: u8, prev1: u8, last: u8, bit_pos: u8, bit: bool) {
        let ctx = make_order3_ctx(prev2, prev1, last);
        let entry = self.table.entry((ctx, bit_pos)).or_default();
        entry.observe(bit);
    }
}

/// Pack three bytes into a u32 context key (prev2 in the high byte).
fn make_order3_ctx(prev2: u8, prev1: u8, last: u8) -> u32 {
    (u32::from(prev2) << 16) | (u32::from(prev1) << 8) | u32::from(last)
}

/// Combined context-mixing model driving the arithmetic coder.
///
/// Wraps six sub-models (order-0, order-1, order-2, order-3, match,
/// run-length) and an adaptive [`Mixer`](crate::mixer::Mixer). The
/// model is stateless across encode/decode — both sides rebuild it
/// identically because all state depends only on already-coded bytes.
///
/// ## Portfolio selection
///
/// Not every deployment needs all six models. [`Portfolio::Fast`]
/// disables order-2/order-3/run-length for speed on inline-metadata
/// workloads; [`Portfolio::Default`] enables order-2 + run-length but
/// not order-3; [`Portfolio::Best`] enables everything (the legacy
/// behaviour). The mixer's per-model weight adapts to whatever subset
/// is active — disabled models contribute a neutral probability that
/// the mixer learns to weight at zero.
pub struct MultiModel {
    portfolio: Portfolio,
    order0: Order0Model,
    order1: Order1Model,
    order2: Order2Model,
    order3: Order3Model,
    matcher: MatchModel,
    runlen: RunLengthModel,
    word: WordModel,
    mixer: crate::mixer::Mixer,
    last_byte: u8,
    prev_byte: u8,
    prev2_byte: u8,
}

/// Subset of sub-models used by a [`MultiModel`].
///
/// | Variant   | Models enabled                                          |
/// |-----------|---------------------------------------------------------|
/// | `Fast`    | order-0, order-1, match (3 models)                      |
/// | `Default` | + order-2, run-length (5 models)                        |
/// | `Best`    | + order-3, word (7 models)                              |
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Portfolio {
    Fast,
    Default,
    Best,
}

impl Portfolio {
    /// Map a compression level (0..=9) to a portfolio.
    ///
    /// | Level | Portfolio |
    /// |-------|-----------|
    /// | 0..=2 | `Fast`    |
    /// | 3..=5 | `Default` |
    /// | 6..=9 | `Best`    |
    #[must_use]
    pub fn from_level(level: u8) -> Self {
        match level {
            0..=2 => Self::Fast,
            3..=5 => Self::Default,
            _ => Self::Best,
        }
    }

    /// True if the order-2 model is included.
    #[must_use]
    pub const fn includes_order2(self) -> bool {
        matches!(self, Self::Default | Self::Best)
    }

    /// True if the order-3 model is included.
    #[must_use]
    pub const fn includes_order3(self) -> bool {
        matches!(self, Self::Best)
    }

    /// True if the run-length model is included.
    #[must_use]
    pub const fn includes_runlen(self) -> bool {
        matches!(self, Self::Default | Self::Best)
    }

    /// True if the word-level model is included.
    #[must_use]
    pub const fn includes_word(self) -> bool {
        matches!(self, Self::Best)
    }

    /// Stable wire-format identifier (stored in the container header).
    #[must_use]
    pub const fn config_id(self) -> u8 {
        match self {
            Self::Fast => 3,
            Self::Default => 4,
            Self::Best => 5,
        }
    }

    /// Inverse of [`config_id`](Self::config_id). Returns `None` for
    /// legacy ids 1 (Phase 1 order-2) and 2 (Phase 2 all-models);
    /// callers handle those explicitly.
    #[must_use]
    pub fn from_config_id(id: u8) -> Option<Self> {
        match id {
            3 => Some(Self::Fast),
            4 => Some(Self::Default),
            5 | 2 => Some(Self::Best),
            _ => None,
        }
    }
}

impl Default for MultiModel {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiModel {
    #[must_use]
    pub fn new() -> Self {
        Self::with_portfolio(Portfolio::Best)
    }

    /// Construct with a specific model [`Portfolio`]. Inactive models
    /// remain in the struct for layout uniformity but contribute a
    /// neutral probability that the mixer's adaptation zeroes out.
    #[must_use]
    pub fn with_portfolio(portfolio: Portfolio) -> Self {
        Self {
            portfolio,
            order0: Order0Model::new(),
            order1: Order1Model::new(),
            order2: Order2Model::new(),
            order3: Order3Model::new(),
            matcher: MatchModel::new(),
            runlen: RunLengthModel::new(),
            word: WordModel::new(),
            mixer: crate::mixer::Mixer::new(),
            last_byte: 0,
            prev_byte: 0,
            prev2_byte: 0,
        }
    }

    /// Portfolio this model was constructed with.
    #[must_use]
    pub fn portfolio(&self) -> Portfolio {
        self.portfolio
    }

    fn collect_probs(&self, bit_pos: u8) -> [u16; NUM_MODELS] {
        // Always-active models.
        let p0 = Order0Model::prob(bit_pos, &self.order0.counters);
        let p1 = Order1Model::prob(self.prev_byte, bit_pos, &self.order1.counters);
        let pm = self.matcher.prob();

        // Conditionally-active models. When disabled, contribute the
        // neutral 50/50 probability so the mixer's stretch() yields 0
        // and the model has zero effect on the mix.
        let p2 = if self.portfolio.includes_order2() {
            self.order2
                .prob_with_context(self.prev_byte, self.last_byte, bit_pos)
        } else {
            DEFAULT_PROB
        };
        let p3 = if self.portfolio.includes_order3() {
            self.order3
                .prob_with_context(self.prev2_byte, self.prev_byte, self.last_byte, bit_pos)
        } else {
            DEFAULT_PROB
        };
        let pr = if self.portfolio.includes_runlen() {
            self.runlen.prob()
        } else {
            DEFAULT_PROB
        };
        let pw = if self.portfolio.includes_word() {
            self.word.prob(bit_pos)
        } else {
            DEFAULT_PROB
        };
        [p0, p1, p2, p3, pm, pr, pw]
    }

    /// Encode one byte MSB-first using the mixed model probability.
    pub fn encode_byte(&mut self, byte: u8, enc: &mut ArithmeticEncoder) {
        // Start a new match search and reset per-byte model state.
        self.matcher.begin_byte(self.prev_byte, self.last_byte);
        self.runlen.begin_byte();
        self.word.begin_byte(self.prev_byte);

        for bit_pos in 0..8u8 {
            let probs = self.collect_probs(bit_pos);
            let mixed = self.mixer.mix(&probs);
            let bit = (byte >> (7 - bit_pos)) & 1 == 1;
            enc.encode_bit(mixed, bit);

            // Update each sub-model with the now-known bit.
            self.order0.update(bit_pos, bit);
            self.order1.update(self.prev_byte, bit_pos, bit);
            self.order2
                .update_with_context(self.prev_byte, self.last_byte, bit_pos, bit);
            self.order3.update_with_context(
                self.prev2_byte,
                self.prev_byte,
                self.last_byte,
                bit_pos,
                bit,
            );
            self.mixer.update(bit);
            self.matcher.update();
            self.runlen.update();
            self.word.update();
        }

        // Finalise: advance byte context and history.
        self.matcher.end_byte(byte);
        self.runlen.end_byte(byte);
        self.word.end_byte(byte);
        self.prev2_byte = self.prev_byte;
        self.prev_byte = self.last_byte;
        self.last_byte = byte;
    }

    /// Decode one byte MSB-first using the mixed model probability.
    pub fn decode_byte(&mut self, dec: &mut ArithmeticDecoder) -> u8 {
        self.matcher.begin_byte(self.prev_byte, self.last_byte);
        self.runlen.begin_byte();
        self.word.begin_byte(self.prev_byte);

        let mut byte: u8 = 0;
        for bit_pos in 0..8u8 {
            let probs = self.collect_probs(bit_pos);
            let mixed = self.mixer.mix(&probs);
            let bit = dec.decode_bit(mixed);

            self.order0.update(bit_pos, bit);
            self.order1.update(self.prev_byte, bit_pos, bit);
            self.order2
                .update_with_context(self.prev_byte, self.last_byte, bit_pos, bit);
            self.order3.update_with_context(
                self.prev2_byte,
                self.prev_byte,
                self.last_byte,
                bit_pos,
                bit,
            );
            self.mixer.update(bit);
            self.matcher.update();
            self.runlen.update();
            self.word.update();

            if bit {
                byte |= 1 << (7 - bit_pos);
            }
        }

        self.matcher.end_byte(byte);
        self.runlen.end_byte(byte);
        self.word.end_byte(byte);
        self.prev2_byte = self.prev_byte;
        self.prev_byte = self.last_byte;
        self.last_byte = byte;
        byte
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
mod tests {
    use super::*;

    fn round_trip(input: &[u8], label: &str) {
        let mut enc = ArithmeticEncoder::new();
        let mut model = Order2Model::new();
        for &b in input {
            model.encode_byte(b, &mut enc);
        }
        let bytes = enc.finish();

        let mut dec = ArithmeticDecoder::new(&bytes);
        let mut model = Order2Model::new();
        let mut out = Vec::with_capacity(input.len());
        for _ in 0..input.len() {
            out.push(model.decode_byte(&mut dec));
        }
        assert_eq!(out, input, "{label}: round-trip mismatch");
    }

    #[test]
    fn round_trip_empty() {
        round_trip(b"", "empty");
    }

    #[test]
    fn round_trip_single_byte() {
        round_trip(b"A", "single-byte");
    }

    #[test]
    fn round_trip_text() {
        let text = b"The quick brown fox jumps over the lazy dog. ".repeat(10);
        round_trip(&text, "text");
    }

    #[test]
    fn round_trip_binary_sequence() {
        let data: Vec<u8> = (0..=u8::MAX).cycle().take(1000).collect();
        round_trip(&data, "binary");
    }

    #[test]
    fn round_trip_ff_runs() {
        let mut data = vec![0xFFu8; 1024];
        data.extend_from_slice(&vec![0x00u8; 512]);
        data.extend_from_slice(&vec![0xFFu8; 512]);
        round_trip(&data, "ff-runs");
    }

    #[test]
    fn round_trip_repeated_phrase() {
        let phrase = b"all good coders write tests ";
        let data = phrase.repeat(64);
        round_trip(&data, "repeated-phrase");
    }

    #[test]
    fn compresses_repetitive_text() {
        // Use a longer input so the adaptive model has time to learn.
        let data = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".repeat(20);
        let mut enc = ArithmeticEncoder::new();
        let mut model = Order2Model::new();
        for &b in &data {
            model.encode_byte(b, &mut enc);
        }
        let bytes = enc.finish();
        assert!(
            bytes.len() < data.len(),
            "expected compression but got {} bytes for {} input",
            bytes.len(),
            data.len()
        );
    }

    #[test]
    fn compression_ratio_on_text() {
        let text = b"The quick brown fox jumps over the lazy dog. ".repeat(50);
        let mut enc = ArithmeticEncoder::new();
        let mut model = Order2Model::new();
        for &b in &text {
            model.encode_byte(b, &mut enc);
        }
        let bytes = enc.finish();
        let ratio = bytes.len() as f64 / text.len() as f64;
        eprintln!(
            "text: {} bytes -> {} bytes (ratio {:.3})",
            text.len(),
            bytes.len(),
            ratio
        );
        assert!(bytes.len() < text.len(), "expected compression");
    }

    // -----------------------------------------------------------------------
    // MultiModel tests.
    // -----------------------------------------------------------------------

    fn multi_round_trip(input: &[u8], label: &str) {
        let mut enc = ArithmeticEncoder::new();
        let mut model = MultiModel::new();
        for &b in input {
            model.encode_byte(b, &mut enc);
        }
        let bytes = enc.finish();

        let mut dec = ArithmeticDecoder::new(&bytes);
        let mut model = MultiModel::new();
        let mut out = Vec::with_capacity(input.len());
        for _ in 0..input.len() {
            out.push(model.decode_byte(&mut dec));
        }
        assert_eq!(out, input, "{label}: round-trip mismatch");
    }

    #[test]
    fn multi_round_trip_empty() {
        multi_round_trip(b"", "empty");
    }

    #[test]
    fn multi_round_trip_single_byte() {
        multi_round_trip(b"A", "single-byte");
    }

    #[test]
    fn multi_round_trip_text_repetitive() {
        let text = b"The quick brown fox jumps over the lazy dog. ".repeat(20);
        multi_round_trip(&text, "text-repetitive");
    }

    #[test]
    fn multi_round_trip_binary_sequence() {
        let data: Vec<u8> = (0..=u8::MAX).cycle().take(1000).collect();
        multi_round_trip(&data, "binary");
    }

    #[test]
    fn multi_round_trip_ff_runs() {
        let mut data = vec![0xFFu8; 1024];
        data.extend_from_slice(&vec![0x00u8; 512]);
        data.extend_from_slice(&vec![0xFFu8; 512]);
        multi_round_trip(&data, "ff-runs");
    }

    #[test]
    fn multi_round_trip_source_code() {
        // A small source-code snippet — varied byte distribution, repetition
        // at the word level but not at the byte level.
        let code = b"fn main() {\n    let x = vec![1, 2, 3, 4, 5];\n    for i in &x {\n        println!(\"{}\", i);\n    }\n}\n".repeat(8);
        multi_round_trip(&code, "source-code");
    }

    #[test]
    fn multi_round_trip_diverse_text() {
        // Deliberately non-repetitive prose.
        let prose = b"Compression is the art of representing information using fewer bits \
than the naive encoding. Arithmetic coding achieves the entropy limit by \
mapping each symbol to a sub-range of the unit interval proportional to \
its probability. Context mixing extends this idea by combining multiple \
probability estimates, each capturing a different statistical regularity, \
into a single prediction. Logistic mixing in the log-odds domain lets \
models contribute additively, and gradient-based weight adaptation lets \
the codec shift influence toward whichever model is currently most \
accurate. The result is a single probability per bit that is sharper \
than any individual model could produce.";
        multi_round_trip(prose, "diverse-prose");
    }

    #[test]
    fn multi_round_trip_english_words() {
        // Diverse English vocabulary — stresses order-1/order-0 models.
        let words = b"apple banana cherry date elderberry fig grape honeydew \
ice jelly kiwi lemon mango nectarine orange papaya quince raspberry \
strawberry tangerine ugli vanilla watermelon yam zucchini apricot \
blueberry coconut damson eggplant feijoa guava huckleberry jackfruit \
kumquat lime mulberry olive peach pear plum raisin tamarind ";
        multi_round_trip(words, "english-words");
    }

    #[test]
    fn multi_is_deterministic() {
        let input = b"deterministic compression for content addressing";
        let mut a = Vec::new();
        for run in 0..3 {
            let mut enc = ArithmeticEncoder::new();
            let mut model = MultiModel::new();
            for &b in input {
                model.encode_byte(b, &mut enc);
            }
            let bytes = enc.finish();
            if run == 0 {
                a = bytes;
            } else {
                assert_eq!(a, bytes, "non-deterministic run {run}");
            }
        }
    }

    /// Phase 2 should compress diverse prose better than Phase 1's order-2
    /// model alone.
    #[test]
    fn multi_beats_phase1_on_diverse_text() {
        let prose = b"Compression is the art of representing information using fewer bits \
than the naive encoding. Arithmetic coding achieves the entropy limit by \
mapping each symbol to a sub-range of the unit interval proportional to \
its probability. Context mixing extends this idea by combining multiple \
probability estimates, each capturing a different statistical regularity, \
into a single prediction. Logistic mixing in the log-odds domain lets \
models contribute additively, and gradient-based weight adaptation lets \
the codec shift influence toward whichever model is currently most \
accurate. The result is a single probability per bit that is sharper \
than any individual model could produce."
            .repeat(2);

        // Phase 1.
        let mut enc1 = ArithmeticEncoder::new();
        let mut m1 = Order2Model::new();
        for &b in &prose {
            m1.encode_byte(b, &mut enc1);
        }
        let bytes1 = enc1.finish();

        // Phase 2.
        let mut enc2 = ArithmeticEncoder::new();
        let mut m2 = MultiModel::new();
        for &b in &prose {
            m2.encode_byte(b, &mut enc2);
        }
        let bytes2 = enc2.finish();

        let r1 = bytes1.len() as f64 / prose.len() as f64;
        let r2 = bytes2.len() as f64 / prose.len() as f64;
        eprintln!(
            "phase1 ratio {:.4} ({} bytes); phase2 ratio {:.4} ({} bytes)",
            r1,
            bytes1.len(),
            r2,
            bytes2.len()
        );
        assert!(
            bytes2.len() < bytes1.len(),
            "phase2 ({}) should beat phase1 ({}) on diverse text",
            bytes2.len(),
            bytes1.len()
        );
    }
}
