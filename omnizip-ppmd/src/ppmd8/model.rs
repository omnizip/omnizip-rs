//! PPMd8 (PPMdI) prediction model with memory management.
//!
//! Ported from the Ruby reference at
//! `omnizip/lib/omnizip/algorithms/ppmd8/{model,context,constants,restoration_method}.rb`.
//!
//! ## PPMd8-specific features (vs PPMd7)
//!
//! * **RESTART restoration** — when the node budget is exhausted, clear the
//!   entire context trie and resume from a halved model. Simpler than
//!   CUT_OFF, and guarantees a hard memory ceiling.
//! * **Glue counting** — every context node carries a `glue_count` that
//!   increments each time the node is visited. This field is wired in for
//!   CUT_OFF support (a future restoration method can prune low-glue nodes
//!   first). RESTART currently uses the global node-count trigger.
//! * **Run-length encoding (RLE)** — after each byte, the encoder emits a
//!   flag bit ("does the current run continue?"). When a run of identical
//!   bytes reaches `RLE_THRESHOLD`, the flag is set and a gamma-coded run
//!   length follows, absorbing the remaining identical bytes in one chunk.
//!   This keeps encoder and decoder perfectly synchronized: both know the
//!   run state at every byte boundary.
//!
//! ## Arithmetic coding
//!
//! A 32-bit-precision binary arithmetic coder (Witten-Neal-Cleary 1987),
//! same family as the PPMd7 model in `../model.rs`. Symbols are coded
//! bit-by-bit via a `(context_hash, bit_position)` state machine, which
//! keeps the implementation simple and fully deterministic.
//!
//! ## Determinism
//!
//! No RNGs, no `HashMap` iteration. The trie is walked by linear scan of
//! `Vec<(u8, child)>` in first-occurrence order — identical across runs.
//! The RLE flag and run length use a fixed gamma code. Same input always
//! produces byte-identical output.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::doc_markdown
)]

// ── Arithmetic coder ────────────────────────────────────────────────

const PRECISION: u32 = 32;
const HALF: u64 = 1u64 << (PRECISION - 1);
const QUARTER: u64 = 1u64 << (PRECISION - 2);
const THREE_Q: u64 = 3 * QUARTER;
const MASK: u32 = u32::MAX;
const PROB_SCALE: u64 = 65536;

pub struct ArithEncoder {
    low: u64,
    high: u64,
    pending_ff: u64,
    out: Vec<u8>,
    bit_buf: u32,
    bit_count: u32,
}

impl ArithEncoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            low: 0,
            high: u64::from(MASK),
            pending_ff: 0,
            out: Vec::new(),
            bit_buf: 0,
            bit_count: 0,
        }
    }

    pub fn encode_bit(&mut self, prob: u16, bit: bool) {
        let range = self.high - self.low + 1;
        let split = self.low + range - (range * u64::from(prob)) / PROB_SCALE;
        if bit {
            self.low = split;
        } else {
            self.high = split - 1;
        }
        loop {
            if self.high < HALF {
                self.emit_bit(false);
            } else if self.low >= HALF {
                self.emit_bit(true);
                self.low -= HALF;
                self.high -= HALF;
            } else if self.low >= QUARTER && self.high < THREE_Q {
                self.pending_ff += 1;
                self.low -= QUARTER;
                self.high -= QUARTER;
            } else {
                break;
            }
            self.low <<= 1;
            self.high = (self.high << 1) | 1;
        }
    }

    fn emit_bit(&mut self, bit: bool) {
        self.push_bit(bit);
        for _ in 0..self.pending_ff {
            self.push_bit(!bit);
        }
        self.pending_ff = 0;
    }

    fn push_bit(&mut self, bit: bool) {
        self.bit_buf = (self.bit_buf << 1) | u32::from(bit);
        self.bit_count += 1;
        if self.bit_count == 8 {
            self.out.push(self.bit_buf as u8);
            self.bit_buf = 0;
            self.bit_count = 0;
        }
    }

    pub fn flush(mut self, out: &mut Vec<u8>) {
        self.pending_ff += 1;
        if self.low >= QUARTER {
            self.emit_bit(true);
        } else {
            self.emit_bit(false);
        }
        while self.bit_count != 0 {
            self.push_bit(false);
        }
        out.extend_from_slice(&self.out);
    }
}

impl Default for ArithEncoder {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ArithDecoder<'a> {
    low: u64,
    high: u64,
    code: u64,
    data: &'a [u8],
    pos: usize,
    bit_buf: u32,
    bit_count: u32,
}

impl<'a> ArithDecoder<'a> {
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        let mut s = Self {
            low: 0,
            high: u64::from(MASK),
            code: 0,
            data,
            pos: 0,
            bit_buf: 0,
            bit_count: 0,
        };
        for _ in 0..PRECISION {
            let b = s.read_bit();
            s.code = (s.code << 1) | u64::from(b);
        }
        s
    }

    fn read_bit(&mut self) -> u8 {
        if self.bit_count == 0 {
            self.bit_buf = u32::from(if self.pos < self.data.len() {
                let b = self.data[self.pos];
                self.pos += 1;
                b
            } else {
                0
            });
            self.bit_count = 8;
        }
        self.bit_count -= 1;
        ((self.bit_buf >> self.bit_count) & 1) as u8
    }

    pub fn decode_bit(&mut self, prob: u16) -> bool {
        let range = self.high - self.low + 1;
        let split = self.low + range - (range * u64::from(prob)) / PROB_SCALE;
        let bit = self.code > split - 1;
        if bit {
            self.low = split;
        } else {
            self.high = split - 1;
        }
        loop {
            if self.high < HALF {
            } else if self.low >= HALF {
                self.low -= HALF;
                self.high -= HALF;
                self.code -= HALF;
            } else if self.low >= QUARTER && self.high < THREE_Q {
                self.low -= QUARTER;
                self.high -= QUARTER;
                self.code -= QUARTER;
            } else {
                break;
            }
            self.low <<= 1;
            self.high = (self.high << 1) | 1;
            self.code = (self.code << 1) | u64::from(self.read_bit());
        }
        bit
    }
}

// ── Bit probability model ───────────────────────────────────────────

#[derive(Clone, Copy)]
struct BitModel {
    n0: u16,
    n1: u16,
}

impl BitModel {
    const fn new() -> Self {
        Self { n0: 1, n1: 1 }
    }
    fn prob1(self) -> u16 {
        let t = u32::from(self.n0) + u32::from(self.n1);
        (((u32::from(self.n1) << 16) + t / 2) / t).clamp(1, 65535) as u16
    }
    fn update(&mut self, bit: bool) {
        if bit {
            self.n1 = self.n1.saturating_add(1);
        } else {
            self.n0 = self.n0.saturating_add(1);
        }
        if self.n0 + self.n1 > 1 << 12 {
            self.n0 = (self.n0 + 1) >> 1;
            self.n1 = (self.n1 + 1) >> 1;
        }
    }
}

// ── Context node with glue counting ─────────────────────────────────

/// A node in the PPMd8 context trie.
///
/// Beyond the standard PPM frequency tracking, PPMd8 nodes carry a
/// `glue_count` — how many times this context has been used as a
/// prediction source. High-glue nodes are valuable and should be
/// preserved during CUT_OFF restoration; low-glue nodes are prunable.
#[derive(Clone, Debug, Default)]
pub struct Ppmd8Context {
    /// Number of times this node was visited as a prediction context.
    pub glue_count: u32,
    /// Child contexts keyed by the extending byte (first-occurrence order).
    children: Vec<(u8, Ppmd8Context)>,
}

impl Ppmd8Context {
    fn child(&self, byte: u8) -> Option<&Ppmd8Context> {
        self.children
            .iter()
            .find_map(|(b, n)| if *b == byte { Some(n) } else { None })
    }

    fn child_mut(&mut self, byte: u8) -> &mut Ppmd8Context {
        if let Some(i) = self.children.iter().position(|(b, _)| *b == byte) {
            &mut self.children[i].1
        } else {
            self.children.push((byte, Ppmd8Context::default()));
            let last = self.children.last_mut().expect("just pushed");
            &mut last.1
        }
    }
}

// ── Constants (ported from ppmd8/constants.rb) ──────────────────────

/// Restoration method: clear the whole tree and start over (default).
pub const RESTORE_METHOD_RESTART: u8 = 0;
/// Restoration method: prune low-glue contexts (reserved — falls back
/// to RESTART for correctness now).
pub const RESTORE_METHOD_CUT_OFF: u8 = 1;
pub const DEFAULT_RESTORE_METHOD: u8 = RESTORE_METHOD_RESTART;

/// Glue count at which a context is considered "heavy" (Ruby:
/// `GLUE_COUNT_THRESHOLD`).
pub const GLUE_COUNT_THRESHOLD: u32 = 255;

/// Run-length encoding activates once a byte repeats this many times.
/// Below this threshold, each byte is coded individually through the
/// trie. Above it, the remaining run is gamma-coded in one chunk.
pub const RLE_THRESHOLD: usize = 4;

/// Maximum additional run length encodable in one RLE chunk
/// (gamma-coded up to 16 bits).
pub const MAX_RUN: usize = 0xFFFF;

/// Default node budget before RESTART fires. At ~40 bytes per node
/// (Vec overhead + children Vec), this caps the trie around 64 MB.
pub const DEFAULT_MAX_NODES: usize = 1_600_000;

// ── PPMd8 model ─────────────────────────────────────────────────────

/// PPMd8 prediction model with RESTART restoration, glue counting, and RLE.
pub struct Ppmd8Model {
    /// Maximum context order (trie depth).
    max_order: usize,
    /// Restoration method selector (RESTART or CUT_OFF).
    restore_method: u8,
    /// Hard ceiling on total context nodes before restoration triggers.
    max_nodes: usize,
    /// Current number of allocated context nodes.
    node_count: usize,
    /// The trie root (empty context).
    root: Ppmd8Context,
    /// Sliding history window (most-recent-byte last).
    history: Vec<u8>,
    /// Flat probability table indexed by `(context_hash, bit_pos)`.
    /// This is where the arithmetic-coder probabilities live — same
    /// design as the PPMd7 model.
    models: Vec<BitModel>,
    table_mask: usize,
    /// Number of RESTART restoration events that have fired.
    restart_count: u32,
    /// Last byte seen (for run detection). `None` before the first byte.
    last_byte: Option<u8>,
    /// Current run length of identical bytes (1 = single occurrence).
    run_length: usize,
    /// Number of RLE chunks emitted (for tests/diagnostics).
    rle_count: u32,
    /// Whether the current run has already been RLE-encoded (prevents
    /// re-encoding while the run continues to grow before the break).
    rle_emitted_for_run: bool,
}

impl Ppmd8Model {
    /// Create with the given order, restoration method, and node budget.
    #[must_use]
    pub fn new(max_order: usize, restore_method: u8, max_nodes: usize) -> Self {
        let slots = Self::table_slots(max_nodes);
        Self {
            max_order,
            restore_method,
            max_nodes,
            node_count: 0,
            root: Ppmd8Context::default(),
            history: Vec::new(),
            models: vec![BitModel::new(); slots],
            table_mask: slots - 1,
            restart_count: 0,
            last_byte: None,
            run_length: 0,
            rle_count: 0,
            rle_emitted_for_run: false,
        }
    }

    /// Default model: given order, RESTART restoration, ~64 MB budget.
    #[must_use]
    pub fn default_for(max_order: usize) -> Self {
        Self::new(max_order, DEFAULT_RESTORE_METHOD, DEFAULT_MAX_NODES)
    }

    fn table_slots(max_nodes: usize) -> usize {
        let based = max_nodes.min(1 << 20);
        (based / 4).next_power_of_two().clamp(1 << 12, 1 << 20)
    }

    /// Number of RESTART restoration events that have fired.
    #[must_use]
    pub fn restart_events(&self) -> u32 {
        self.restart_count
    }

    /// Number of RLE chunks that have been emitted.
    #[must_use]
    pub fn rle_events(&self) -> u32 {
        self.rle_count
    }

    /// The configured restoration method (RESTART or CUT_OFF).
    #[must_use]
    pub fn restore_method(&self) -> u8 {
        self.restore_method
    }

    /// The maximum context order.
    #[must_use]
    pub fn max_order(&self) -> usize {
        self.max_order
    }

    /// The node budget before restoration triggers.
    #[must_use]
    pub fn max_nodes(&self) -> usize {
        self.max_nodes
    }

    // ── Context hash (for the probability table) ───────────────────

    /// Copy the last `max_order` bytes of history into a stack buffer.
    /// Returns `(buffer, valid_len)`. Bounded to 16 bytes, so no heap
    /// allocation. Used to decouple reading the context from mutating
    /// the trie (borrow checker).
    fn context_window(&self) -> ([u8; 16], usize) {
        let mut buf = [0u8; 16];
        let len = self.history.len().min(self.max_order);
        if len == 0 {
            return (buf, 0);
        }
        let start = self.history.len() - len;
        buf[..len].copy_from_slice(&self.history[start..]);
        (buf, len)
    }

    fn ctx_hash(&self) -> u32 {
        let len = self.history.len().min(self.max_order);
        if len == 0 {
            return 0;
        }
        let start = self.history.len() - len;
        let mut h: u32 = 5381;
        for &b in &self.history[start..] {
            h = h.wrapping_mul(33).wrapping_add(u32::from(b));
        }
        h
    }

    // ── Trie accounting (glue counts + node budget) ────────────────

    /// Descend into the trie along the current history, incrementing
    /// glue counts and creating nodes as needed. Nodes are created up
    /// to `max_order` deep. Takes the history as a separate parameter
    /// to avoid borrowing `self` immutably while mutating the trie.
    fn descend_and_glue(&mut self, ctx: &[u8]) {
        let depth = ctx.len().min(self.max_order);
        let mut node: &mut Ppmd8Context = &mut self.root;
        node.glue_count = node.glue_count.saturating_add(1);
        if depth == 0 {
            return;
        }
        for &b in &ctx[ctx.len() - depth..] {
            let is_new = node.child(b).is_none();
            node = node.child_mut(b);
            node.glue_count = node.glue_count.saturating_add(1);
            if is_new {
                self.node_count += 1;
            }
        }
    }

    // ── Memory management: RESTART restoration ─────────────────────

    fn check_memory(&mut self) {
        if self.node_count >= self.max_nodes {
            self.do_restore();
        }
    }

    fn do_restore(&mut self) {
        // CUT_OFF's full per-node prune algorithm (Shkarin's frequency
        // halving) is complex; for now both methods fall back to RESTART.
        // The restore_method field preserves the selection for a future
        // CUT_OFF implementation.
        let _ = self.restore_method;
        self.restart_tree();
    }

    /// RESTART: clear the entire trie and halve probability tables.
    /// The history window is preserved so contexts can be rebuilt.
    fn restart_tree(&mut self) {
        self.root = Ppmd8Context::default();
        self.node_count = 0;
        for m in &mut self.models {
            m.n0 = (m.n0 + 1) >> 1;
            m.n1 = (m.n1 + 1) >> 1;
        }
        self.restart_count += 1;
    }

    // ── Bit-level byte coding ──────────────────────────────────────

    fn encode_byte_bits(&mut self, enc: &mut ArithEncoder, byte: u8) {
        let ctx = self.ctx_hash();
        for bp in (0..8u32).rev() {
            let bit = ((byte >> bp) & 1) == 1;
            let idx = ((ctx.wrapping_mul(8).wrapping_add(bp)) as usize) & self.table_mask;
            let prob = self.models[idx].prob1();
            enc.encode_bit(prob, bit);
            self.models[idx].update(bit);
        }
    }

    fn decode_byte_bits(&mut self, dec: &mut ArithDecoder) -> u8 {
        let ctx = self.ctx_hash();
        let mut byte = 0u8;
        for bp in (0..8u32).rev() {
            let idx = ((ctx.wrapping_mul(8).wrapping_add(bp)) as usize) & self.table_mask;
            let prob = self.models[idx].prob1();
            let bit = dec.decode_bit(prob);
            if bit {
                byte |= 1 << bp;
            }
            self.models[idx].update(bit);
        }
        byte
    }

    // ── Gamma-coded run length (RLE) ───────────────────────────────

    /// Encode a positive integer using Elias gamma coding via the
    /// arithmetic coder with a fixed 50% probability. Deterministic,
    /// no dictionaries, prefix-free.
    fn encode_gamma(enc: &mut ArithEncoder, n: usize) {
        debug_assert!(n >= 1, "gamma code requires n >= 1");
        let mut bits = 0u32;
        let mut v = n;
        while v > 1 {
            bits += 1;
            v >>= 1;
        }
        // `bits` zero bits, then `bits+1` bits of n (MSB first, leading 1).
        for _ in 0..bits {
            enc.encode_bit(32768, false);
        }
        for i in (0..=bits).rev() {
            let bit = ((n >> i) & 1) == 1;
            enc.encode_bit(32768, bit);
        }
    }

    fn decode_gamma(dec: &mut ArithDecoder) -> usize {
        let mut bits = 0u32;
        while !dec.decode_bit(32768) {
            bits += 1;
        }
        let mut n = 1usize;
        for _ in 0..bits {
            n = (n << 1) | usize::from(dec.decode_bit(32768));
        }
        n
    }

    // ── Run-length state machine ──────────────────────────────────

    /// Update run tracking given the byte just processed.
    fn update_run_state(&mut self, byte: u8) {
        if Some(byte) == self.last_byte {
            self.run_length += 1;
        } else {
            self.run_length = 1;
            self.last_byte = Some(byte);
            self.rle_emitted_for_run = false;
        }
    }

    /// After coding a byte, check whether the current run has crossed
    /// the RLE threshold. If so and we haven't emitted an RLE chunk for
    /// this run yet, emit a marker bit: 0 = run ends here (no extra
    /// bytes), 1 = RLE chunk follows with a gamma-coded count.
    ///
    /// Returns the number of additional bytes absorbed by the RLE
    /// chunk (the caller's loop must skip that many input bytes).
    fn maybe_emit_rle(&mut self, enc: &mut ArithEncoder, remaining_input: &[u8]) -> usize {
        if self.run_length <= RLE_THRESHOLD || self.rle_emitted_for_run {
            return 0;
        }
        // Count how many more identical bytes follow in the input.
        // `remaining_input` starts at the byte AFTER the one just coded.
        let mut extra = 0usize;
        for &b in remaining_input {
            if Some(b) == self.last_byte && extra < MAX_RUN {
                extra += 1;
            } else {
                break;
            }
        }
        // Always emit a marker bit so the decoder knows whether to
        // read a gamma-coded count. 0 = no continuation, 1 = RLE chunk.
        if extra == 0 {
            enc.encode_bit(32768, false);
        } else {
            enc.encode_bit(32768, true);
            Self::encode_gamma(enc, extra);
            self.rle_count += 1;
        }
        self.rle_emitted_for_run = true;
        self.run_length += extra;
        extra
    }

    /// Decoder side: after decoding a byte, check for an RLE marker.
    /// Returns the number of extra identical bytes to replay.
    fn maybe_decode_rle(&mut self, dec: &mut ArithDecoder) -> usize {
        if self.run_length <= RLE_THRESHOLD || self.rle_emitted_for_run {
            return 0;
        }
        // The encoder only emits a marker if extra > 0. We decode a bit
        // with 50% probability: 1 = RLE chunk follows, 0 = no RLE.
        if !dec.decode_bit(32768) {
            self.rle_emitted_for_run = true; // no more RLE for this run
            return 0;
        }
        let extra = Self::decode_gamma(dec);
        self.rle_count += 1;
        self.rle_emitted_for_run = true;
        self.run_length += extra;
        extra
    }

    // ── Public encode/decode ──────────────────────────────────────

    /// Encode the byte stream. Handles RLE internally: once a run of
    /// identical bytes reaches `RLE_THRESHOLD`, the remaining identical
    /// bytes are absorbed into a gamma-coded chunk.
    pub fn encode_stream(&mut self, enc: &mut ArithEncoder, input: &[u8]) {
        let mut i = 0;
        while i < input.len() {
            let byte = input[i];
            self.update_run_state(byte);
            self.encode_byte_bits(enc, byte);
            // Copy the bounded context window to avoid borrowing self
            // while mutating the trie.
            let (ctx, ctx_len) = self.context_window();
            self.descend_and_glue(&ctx[..ctx_len]);
            self.history.push(byte);
            self.check_memory();

            // After coding, see if RLE should fire. `maybe_emit_rle`
            // needs to peek at the remaining input to count the run.
            let remaining = &input[i + 1..];
            let absorbed = self.maybe_emit_rle(enc, remaining);
            // The absorbed bytes still need to go through history so the
            // model state stays in sync with the decoder, which will
            // replay them without trie updates.
            for _ in 0..absorbed {
                self.history.push(byte);
            }
            i += 1 + absorbed;
        }
    }

    /// Decode exactly `len` bytes from the decoder.
    pub fn decode_stream(&mut self, dec: &mut ArithDecoder, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let byte = self.decode_byte_bits(dec);
            self.update_run_state(byte);
            let (ctx, ctx_len) = self.context_window();
            self.descend_and_glue(&ctx[..ctx_len]);
            self.history.push(byte);
            self.check_memory();
            out.push(byte);

            // Check for RLE replay.
            let extra = self.maybe_decode_rle(dec);
            for _ in 0..extra {
                if out.len() >= len {
                    break;
                }
                out.push(byte);
                self.history.push(byte);
            }
        }
        out
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(data: &[u8], order: usize) {
        let mut model = Ppmd8Model::default_for(order);
        let mut buf = Vec::new();
        {
            let mut enc = ArithEncoder::new();
            model.encode_stream(&mut enc, data);
            enc.flush(&mut buf);
        }
        let mut model2 = Ppmd8Model::default_for(order);
        let mut dec = ArithDecoder::new(&buf);
        let out = model2.decode_stream(&mut dec, data.len());
        assert_eq!(
            out,
            data,
            "round-trip failed at order {order} (len={})",
            data.len()
        );
    }

    #[test]
    fn round_trip_single_byte() {
        round_trip(b"A", 4);
    }

    #[test]
    fn round_trip_two_bytes() {
        round_trip(b"AB", 4);
    }

    #[test]
    fn round_trip_text() {
        let text = b"The quick brown fox jumps over the lazy dog. ".repeat(10);
        round_trip(&text, 4);
    }

    #[test]
    fn round_trip_all_bytes() {
        let data: Vec<u8> = (0..=255u16).map(|i| i as u8).collect();
        round_trip(&data, 4);
    }

    #[test]
    fn round_trip_long_text() {
        let text: Vec<u8> =
            b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(200);
        round_trip(&text, 6);
    }

    #[test]
    fn round_trip_repeated_byte() {
        let data = vec![b'A'; 1000];
        round_trip(&data, 4);
    }

    #[test]
    fn round_trip_repetitive_lines() {
        let line = b"the quick brown fox\n";
        let data: Vec<u8> = line.repeat(500);
        round_trip(&data, 6);
    }

    #[test]
    fn round_trip_empty() {
        round_trip(b"", 4);
    }

    #[test]
    fn round_trip_mixed_with_runs() {
        // Text followed by a long run of zeros, then more text.
        let mut data = Vec::new();
        data.extend_from_slice(b"hello world ");
        data.extend_from_slice(&[0u8; 200]);
        data.extend_from_slice(b" goodbye world ");
        data.extend_from_slice(&[b'X'; 50]);
        round_trip(&data, 4);
    }

    #[test]
    fn round_trip_binary_zero_inclusive() {
        let mut data = Vec::new();
        for _ in 0..50 {
            data.extend_from_slice(&[0u8; 5]);
            data.push(255);
        }
        round_trip(&data, 4);
    }

    #[test]
    fn rle_detected_on_long_run() {
        let data = vec![0u8; 100];
        let mut model = Ppmd8Model::default_for(4);
        let mut enc = ArithEncoder::new();
        model.encode_stream(&mut enc, &data);
        enc.flush(&mut Vec::new());
        assert!(
            model.rle_events() >= 1,
            "RLE should fire on long run of zeros"
        );
    }

    #[test]
    fn rle_not_triggered_on_short_run() {
        // A run of exactly RLE_THRESHOLD should not fire RLE.
        let data = vec![b'A'; RLE_THRESHOLD];
        let mut model = Ppmd8Model::default_for(4);
        let mut enc = ArithEncoder::new();
        model.encode_stream(&mut enc, &data);
        enc.flush(&mut Vec::new());
        assert_eq!(model.rle_events(), 0, "RLE should not fire at threshold");
    }

    #[test]
    fn memory_limit_triggers_restart() {
        // Tiny node budget forces RESTART on modest diverse input.
        let max_nodes = 50;
        let mut model = Ppmd8Model::new(4, RESTORE_METHOD_RESTART, max_nodes);
        let mut enc = ArithEncoder::new();
        let data: Vec<u8> = (0..2000u32).map(|i| ((i * 37) & 0xFF) as u8).collect();
        model.encode_stream(&mut enc, &data);
        enc.flush(&mut Vec::new());
        assert!(
            model.restart_events() >= 1,
            "RESTART should fire under memory pressure"
        );
    }

    #[test]
    fn determinism_same_input_same_output() {
        let mk = || {
            let mut m = Ppmd8Model::default_for(4);
            let mut buf = Vec::new();
            let mut enc = ArithEncoder::new();
            m.encode_stream(
                &mut enc,
                b"hello world test data with some repetition repetition repetition",
            );
            enc.flush(&mut buf);
            buf
        };
        assert_eq!(mk(), mk(), "non-deterministic output");
    }

    #[test]
    fn restart_preserves_round_trip() {
        // Even with aggressive RESTART, round-trip must hold.
        let max_nodes = 30;
        let data: Vec<u8> = (0..3000u32).map(|i| ((i * 73) & 0xFF) as u8).collect();

        let mut model = Ppmd8Model::new(4, RESTORE_METHOD_RESTART, max_nodes);
        let mut buf = Vec::new();
        {
            let mut enc = ArithEncoder::new();
            model.encode_stream(&mut enc, &data);
            enc.flush(&mut buf);
        }
        assert!(model.restart_events() > 0);

        let mut model2 = Ppmd8Model::new(4, RESTORE_METHOD_RESTART, max_nodes);
        let mut dec = ArithDecoder::new(&buf);
        let out = model2.decode_stream(&mut dec, data.len());
        assert_eq!(out, data);
        assert_eq!(
            model2.restart_events(),
            model.restart_events(),
            "decoder must fire RESTART at the same points as encoder"
        );
    }

    #[test]
    fn gamma_round_trip() {
        // Verify the gamma code is symmetric.
        for &n in &[
            1usize, 2, 3, 4, 5, 7, 8, 15, 16, 31, 100, 255, 256, 1000, 65535,
        ] {
            let mut enc = ArithEncoder::new();
            let mut buf = Vec::new();
            Ppmd8Model::encode_gamma(&mut enc, n);
            enc.flush(&mut buf);
            let mut dec = ArithDecoder::new(&buf);
            let got = Ppmd8Model::decode_gamma(&mut dec);
            assert_eq!(got, n, "gamma round-trip failed for {n}");
        }
    }
}
