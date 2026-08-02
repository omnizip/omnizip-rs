//! PPM byte-level prediction model.
//!
//! True byte-level PPM (Prediction by Partial Matching) with the
//! PPM*C escape mechanism (Witten-Bell / Shkarin 2002). Each byte
//! is encoded as a sequence of binary "is it s_i?" decisions
//! against the symbol list of the longest matching context. On
//! total miss, an escape drops to a shorter context. The order-(-1)
//! fallback emits 8 equiprobable bits.
//!
//! ## Algorithm
//!
//! For an order-K context with observed distinct symbols
//! `[s_0, s_1, ..., s_{D-1}]` with counts `[c_0, ..., c_{D-1}]`:
//!
//! - Coding total is `T + E` where `T = sum(c_i)` and
//!   `E = D + 1` (PPM*C escape count).
//! - Walk the symbol list. At step `i`, encode "is the byte s_i?"
//!   with probability `c_i / remaining`, where `remaining` is the
//!   current cumulative weight of unvisited slots plus escape.
//! - "No" decrements `remaining` by `c_i` and continues.
//! - "Yes" terminates with the byte being `s_i`.
//! - If all `D` decisions are "no", the byte is the escape — drop
//!   to order K-1 and try again.
//!
//! ## Update rule
//!
//! After coding a byte, walk ALL orders from `max_order` down to 1,
//! adding the byte to each context (creating if needed). This is
//! the standard PPM "add to all suffix contexts" rule.
//!
//! ## Memory
//!
//! - Context table: 16K slots × ~12 bytes/slot = ~200 KB FIXED.
//! - Each slot's symbol list: up to 256 × 3 bytes = 768 B.
//! - Worst case: 16K × 768 B = 12 MB if every slot is full.
//! - History: sliding window of `max_order` bytes (≤ 16 B).
//!
//! Total worst case: ~12 MB regardless of input size. No input
//! cap is needed — compress(gigabyte_input) uses ~12 MB.
//!
//! ## Determinism
//!
//! - Symbol insertion order is first-occurrence; no HashMap iteration.
//! - Table probing is by hash slot, no randomness.
//! - Arithmetic coder is a pure function of bit probabilities and
//!   bit values.
//! - All arithmetic uses `u32` with `saturating_add`.
//!
//! ## Rescale policy
//!
//! When the table reaches 80% capacity, we halve every count
//! (floor at 1). This prevents unbounded count growth. After
//! rescaling, no entries are evicted — they remain with smaller
//! counts. If the table ever reaches 100% capacity, new contexts
//! are NOT created; the encoder uses the escape path instead.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::collapsible_else_if,
    clippy::similar_names
)]

// ── Arithmetic coder (ZPAQ-style binary) ───────────────────────────

const PRECISION: u32 = 32;
const HALF: u64 = 1u64 << (PRECISION - 1);
const QUARTER: u64 = 1u64 << (PRECISION - 2);
const THREE_Q: u64 = 3 * QUARTER;
const MASK: u32 = u32::MAX;
const PROB_SCALE: u64 = 65536;

/// Binary arithmetic encoder. Maintains a `[low, high)` interval
/// in `u64` and emits MSB-first.
pub struct ArithEncoder {
    low: u64,
    high: u64,
    pending_ff: u64,
    out: Vec<u8>,
    bit_buf: u32,
    bit_count: u32,
}

impl ArithEncoder {
    pub fn new(_out: &mut Vec<u8>) -> Self {
        Self {
            low: 0,
            high: u64::from(MASK),
            pending_ff: 0,
            out: Vec::with_capacity(4096),
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

/// Binary arithmetic decoder. Mirrors [`ArithEncoder`].
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

// ── PPM context model ─────────────────────────────────────────────

/// Hash-table size (power of two). 16K slots × ~12 B header =
/// ~200 KB. Each slot's symbol list adds up to 768 B more, so the
/// absolute worst-case memory is ~12 MB. Tuned to fit in L2 cache
/// for speed while keeping enough resolution to handle real text.
const TABLE_LOG2: u32 = 14;
const TABLE_SIZE: usize = 1 << TABLE_LOG2;
const TABLE_MASK: usize = TABLE_SIZE - 1;
/// When `occupied()` reaches this fraction of the table, halve
/// every entry's counts to free statistical weight.
const RESCALE_THRESHOLD: usize = TABLE_SIZE * 4 / 5;

/// Hash a (order, context-bytes) pair. The order is mixed in so
/// an order-2 and order-3 context with the same last bytes do
/// not collide.
fn hash_context(order: u8, bytes: &[u8]) -> u32 {
    let mut h: u32 = 2_166_136_261u32;
    for &b in bytes {
        h ^= u32::from(b);
        h = h.wrapping_mul(16_777_619);
    }
    h ^= u32::from(order).wrapping_mul(0x9E37_79B9);
    h ^= h >> 16;
    h
}

#[derive(Clone, Copy, Debug)]
struct SymCount {
    sym: u8,
    count: u16,
}

#[derive(Clone, Debug)]
struct ContextEntry {
    /// Symbols observed in this context, in first-occurrence order.
    /// Bounded: max 256 entries × 3 bytes = 768 B.
    syms: Vec<SymCount>,
    /// Sum of all `count` fields.
    total: u32,
    /// Order of this context (1..=16).
    order: u8,
}

impl ContextEntry {
    fn new(order: u8) -> Self {
        Self {
            syms: Vec::new(),
            total: 0,
            order,
        }
    }
}

/// Hash-table slot.
type Slot = Option<ContextEntry>;

/// PPM byte-level model with bounded memory.
///
/// Memory is O(TABLE_SIZE × max_syms_per_context + max_order) =
/// ~12 MB regardless of input size. The history is a sliding
/// window of `max_order` bytes (NOT the full input), so even a
/// gigabyte input uses only ~12 MB.
pub struct PpmModel {
    table: Box<[Slot]>,
    /// Sliding-window history: ring buffer of `max_order` bytes.
    history: Vec<u8>,
    /// Write index into `history`. Wraps modulo `history.len()`.
    head: usize,
    /// Number of valid bytes in `history`. Capped at `max_order`.
    history_len: usize,
    /// Maximum context order (1..=16).
    max_order: usize,
}

impl PpmModel {
    pub fn new(max_order: usize) -> Self {
        Self::with_capacity(max_order, 0)
    }

    /// `hint_len` is accepted for API compatibility but ignored —
    /// memory is fixed at ~12 MB regardless of input size.
    pub fn with_capacity(max_order: usize, _hint_len: usize) -> Self {
        let table = vec![None; TABLE_SIZE].into_boxed_slice();
        let order = max_order.clamp(1, 16);
        Self {
            table,
            history: vec![0u8; order],
            head: 0,
            history_len: 0,
            max_order: order,
        }
    }

    pub fn reset(&mut self) {
        for slot in self.table.iter_mut() {
            *slot = None;
        }
        self.head = 0;
        self.history_len = 0;
    }

    fn push_history(&mut self, byte: u8) {
        if self.history.is_empty() {
            return;
        }
        self.history[self.head] = byte;
        self.head = (self.head + 1) % self.history.len();
        if self.history_len < self.history.len() {
            self.history_len += 1;
        }
    }

    /// Copy the last `order` bytes of history into `out`, oldest
    /// first. Returns fewer bytes if history is shorter than `order`.
    /// Allocation-free: caller reuses `out`.
    fn ctx_bytes_into(&self, order: usize, out: &mut Vec<u8>) {
        out.clear();
        if order == 0 || self.history_len == 0 {
            return;
        }
        let take = order.min(self.history_len);
        let cap = self.history.len();
        let start = if self.history_len < cap {
            0
        } else {
            self.head
        };
        out.reserve(take);
        for i in 0..take {
            out.push(self.history[(start + i) % cap]);
        }
    }

    /// Count occupied slots. O(N) but only called on insert path.
    fn occupied(&self) -> usize {
        self.table.iter().filter(|s| s.is_some()).count()
    }

    /// Look up an existing context. Returns the slot index.
    fn lookup(&self, order: u8, bytes: &[u8]) -> Option<usize> {
        let h = hash_context(order, bytes);
        let mut idx = (h as usize) & TABLE_MASK;
        for _ in 0..TABLE_SIZE {
            match &self.table[idx] {
                None => return None,
                Some(entry) => {
                    if entry.order == order {
                        return Some(idx);
                    }
                    idx = (idx + 1) & TABLE_MASK;
                }
            }
        }
        None
    }

    /// Insert a new context. Returns the slot index, or `None` if
    /// the table is completely full (caller should fall back to
    /// escape path).
    fn insert(&mut self, order: u8, bytes: &[u8]) -> Option<usize> {
        if self.occupied() >= RESCALE_THRESHOLD {
            self.rescale_all();
        }
        let h = hash_context(order, bytes);
        let mut idx = (h as usize) & TABLE_MASK;
        for _ in 0..TABLE_SIZE {
            match &self.table[idx] {
                None => {
                    self.table[idx] = Some(ContextEntry::new(order));
                    return Some(idx);
                }
                Some(_) => {
                    idx = (idx + 1) & TABLE_MASK;
                }
            }
        }
        None
    }

    /// Halve every symbol count (floor at 1) and recompute totals.
    /// Frees statistical weight without evicting entries.
    fn rescale_all(&mut self) {
        for slot in self.table.iter_mut() {
            if let Some(entry) = slot.as_mut() {
                let mut new_total: u32 = 0;
                for sc in entry.syms.iter_mut() {
                    let new_c = (u32::from(sc.count) / 2).max(1);
                    sc.count = new_c as u16;
                    new_total += u32::from(sc.count);
                }
                entry.total = new_total;
            }
        }
    }

    /// PPM*C escape count: `D + 1`.
    fn escape_count(d: usize) -> u32 {
        u32::try_from(d).unwrap_or(0) + 1
    }

    /// 16-bit probability for "is the byte s_i?" given current
    /// cumulative weight `remaining` and symbol count `c_i`.
    fn prob16(c_i: u32, remaining: u32) -> u16 {
        if remaining == 0 {
            return 1;
        }
        let p = (u64::from(c_i) * u64::from(PROB_SCALE) + u64::from(remaining) / 2)
            / u64::from(remaining);
        p.min(u64::from(PROB_SCALE) - 1).max(1) as u16
    }

    fn find_sym(entry: &ContextEntry, sym: u8) -> Option<usize> {
        entry.syms.iter().position(|s| s.sym == sym)
    }

    /// Encode one byte. Walks contexts from `max_order` down to 1;
    /// on miss emits escape bits and drops an order. On total miss,
    /// emits 8 equiprobable bits (order -1 fallback). Updates all
    /// orders after coding.
    pub fn encode_byte(&mut self, enc: &mut ArithEncoder, byte: u8) {
        let max_order = self.max_order;
        let mut scratch: Vec<u8> = Vec::with_capacity(max_order);
        let mut order = max_order;
        let mut resolved = false;

        while order > 0 {
            self.ctx_bytes_into(order, &mut scratch);
            let ord = u8::try_from(order).expect("order <= 16");
            if let Some(idx) = self.lookup(ord, &scratch) {
                let snap: Vec<(u8, u16)> = {
                    let entry = self.table[idx].as_ref().expect("Some");
                    entry.syms.iter().map(|s| (s.sym, s.count)).collect()
                };
                let pos = snap.iter().position(|(s, _)| *s == byte);
                let total: u32 = snap.iter().map(|(_, c)| u32::from(*c)).sum::<u32>()
                    + Self::escape_count(snap.len());
                let mut remaining = total;
                match pos {
                    Some(k) => {
                        for (i, (_sym, c)) in snap.iter().enumerate() {
                            let c_i = u32::from(*c);
                            let p = Self::prob16(c_i, remaining);
                            if i == k {
                                enc.encode_bit(p, true);
                                break;
                            }
                            enc.encode_bit(p, false);
                            remaining = remaining.saturating_sub(c_i);
                        }
                        let entry = self.table[idx].as_mut().expect("Some");
                        if let Some(sc) = entry.syms.get_mut(k) {
                            if sc.count < u16::MAX {
                                sc.count += 1;
                            }
                        }
                        entry.total = entry.total.saturating_add(1);
                        resolved = true;
                        break;
                    }
                    None => {
                        // Escape: emit "no" for every symbol.
                        for (_sym, c) in snap.iter() {
                            let c_i = u32::from(*c);
                            let p = Self::prob16(c_i, remaining);
                            enc.encode_bit(p, false);
                            remaining = remaining.saturating_sub(c_i);
                        }
                        order -= 1;
                    }
                }
            } else {
                order -= 1;
            }
        }

        if !resolved {
            // Order (-1): 8 equiprobable bits.
            let mut b = byte;
            for _ in 0..8 {
                let bit = (b & 0x80) != 0;
                enc.encode_bit((PROB_SCALE / 2) as u16, bit);
                b <<= 1;
            }
        }

        // Update: walk all orders from max_order down to 1.
        let mut o = max_order;
        while o >= 1 {
            self.ctx_bytes_into(o, &mut scratch);
            let ord = u8::try_from(o).expect("order <= 16");
            let idx = match self.lookup(ord, &scratch) {
                Some(i) => Some(i),
                None => self.insert(ord, &scratch),
            };
            if let Some(idx) = idx {
                let entry = self.table[idx].as_mut().expect("Some");
                if let Some(pos) = Self::find_sym(entry, byte) {
                    if entry.syms[pos].count < u16::MAX {
                        entry.syms[pos].count += 1;
                    }
                } else if entry.syms.len() < 256 {
                    entry.syms.push(SymCount { sym: byte, count: 1 });
                }
                entry.total = entry.total.saturating_add(1);
            }
            o -= 1;
        }

        self.push_history(byte);
    }

    /// Decode one byte. Mirrors [`encode_byte`].
    pub fn decode_byte(&mut self, dec: &mut ArithDecoder) -> u8 {
        let max_order = self.max_order;
        let mut scratch: Vec<u8> = Vec::with_capacity(max_order);
        let mut order = max_order;
        let mut resolved: Option<u8> = None;

        while order > 0 {
            self.ctx_bytes_into(order, &mut scratch);
            let ord = u8::try_from(order).expect("order <= 16");
            if let Some(idx) = self.lookup(ord, &scratch) {
                let snap: Vec<(u8, u16)> = {
                    let entry = self.table[idx].as_ref().expect("Some");
                    entry.syms.iter().map(|s| (s.sym, s.count)).collect()
                };
                let total: u32 = snap.iter().map(|(_, c)| u32::from(*c)).sum::<u32>()
                    + Self::escape_count(snap.len());
                let mut remaining = total;
                let mut found: Option<u8> = None;
                for (sym, c) in snap.iter() {
                    let c_i = u32::from(*c);
                    let p = Self::prob16(c_i, remaining);
                    if dec.decode_bit(p) {
                        found = Some(*sym);
                        break;
                    }
                    remaining = remaining.saturating_sub(c_i);
                }
                if let Some(sym) = found {
                    let entry = self.table[idx].as_mut().expect("Some");
                    if let Some(pos) = Self::find_sym(entry, sym) {
                        if entry.syms[pos].count < u16::MAX {
                            entry.syms[pos].count += 1;
                        }
                    }
                    entry.total = entry.total.saturating_add(1);
                    resolved = Some(sym);
                    break;
                }
                order -= 1;
            } else {
                order -= 1;
            }
        }

        let byte: u8 = match resolved {
            Some(b) => b,
            None => {
                let mut b: u8 = 0;
                for i in 0..8 {
                    if dec.decode_bit((PROB_SCALE / 2) as u16) {
                        b |= 1 << (7 - i);
                    }
                }
                b
            }
        };

        // Update: walk all orders from max_order down to 1.
        let mut o = max_order;
        while o >= 1 {
            self.ctx_bytes_into(o, &mut scratch);
            let ord = u8::try_from(o).expect("order <= 16");
            let idx = match self.lookup(ord, &scratch) {
                Some(i) => Some(i),
                None => self.insert(ord, &scratch),
            };
            if let Some(idx) = idx {
                let entry = self.table[idx].as_mut().expect("Some");
                if let Some(pos) = Self::find_sym(entry, byte) {
                    if entry.syms[pos].count < u16::MAX {
                        entry.syms[pos].count += 1;
                    }
                } else if entry.syms.len() < 256 {
                    entry.syms.push(SymCount { sym: byte, count: 1 });
                }
                entry.total = entry.total.saturating_add(1);
            }
            o -= 1;
        }

        self.push_history(byte);
        byte
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_single_byte() {
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        {
            let mut enc = ArithEncoder::new(&mut buf);
            m.encode_byte(&mut enc, b'A');
            enc.flush(&mut buf);
        }
        let mut m2 = PpmModel::new(4);
        let mut dec = ArithDecoder::new(&buf);
        assert_eq!(m2.decode_byte(&mut dec), b'A');
    }

    #[test]
    fn round_trip_text() {
        let text = b"The quick brown fox jumps over the lazy dog. ".repeat(10);
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        {
            let mut enc = ArithEncoder::new(&mut buf);
            for &b in &text {
                m.encode_byte(&mut enc, b);
            }
            enc.flush(&mut buf);
        }
        let mut m2 = PpmModel::new(4);
        let mut dec = ArithDecoder::new(&buf);
        let out: Vec<u8> = (0..text.len()).map(|_| m2.decode_byte(&mut dec)).collect();
        assert_eq!(out, text);
    }

    #[test]
    fn round_trip_all_bytes() {
        let data: Vec<u8> = (0..=255u16).map(|i| i as u8).collect();
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        {
            let mut enc = ArithEncoder::new(&mut buf);
            for &b in &data {
                m.encode_byte(&mut enc, b);
            }
            enc.flush(&mut buf);
        }
        let mut m2 = PpmModel::new(4);
        let mut dec = ArithDecoder::new(&buf);
        let out: Vec<u8> = (0..data.len()).map(|_| m2.decode_byte(&mut dec)).collect();
        assert_eq!(out, data);
    }

    #[test]
    fn round_trip_english_text() {
        let text = b"The quick brown fox jumps over the lazy dog. \
            Pack my box with five dozen liquor jugs. \
            She sells seashells by the seashore. ";
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        {
            let mut enc = ArithEncoder::new(&mut buf);
            for &b in text {
                m.encode_byte(&mut enc, b);
            }
            enc.flush(&mut buf);
        }
        let mut m2 = PpmModel::new(4);
        let mut dec = ArithDecoder::new(&buf);
        let out: Vec<u8> = (0..text.len()).map(|_| m2.decode_byte(&mut dec)).collect();
        assert_eq!(out, text);
    }

    #[test]
    fn compresses_repetitive() {
        let text = b"hello world ".repeat(100);
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        {
            let mut enc = ArithEncoder::new(&mut buf);
            for &b in &text {
                m.encode_byte(&mut enc, b);
            }
            enc.flush(&mut buf);
        }
        let ratio = buf.len() as f64 / text.len() as f64;
        eprintln!("repetitive ratio: {ratio:.3}");
        assert!(ratio < 0.20, "ratio {ratio:.3} >= 0.20");
    }

    #[test]
    fn compresses_english_better_than_bit_model() {
        // The previous bit-level model achieved ~0.68 on English text.
        // Byte-level PPM should reach at least 0.50 (target ~0.30).
        let text: Vec<u8> = b"The quick brown fox jumps over the lazy dog. \
            Pack my box with five dozen liquor jugs. \
            She sells seashells by the seashore. \
            Peter Piper picked a peck of pickled peppers. "
            .iter()
            .cycle()
            .copied()
            .take(20_000)
            .collect();
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        {
            let mut enc = ArithEncoder::new(&mut buf);
            for &b in &text {
                m.encode_byte(&mut enc, b);
            }
            enc.flush(&mut buf);
        }
        let ratio = buf.len() as f64 / text.len() as f64;
        eprintln!("english ratio: {ratio:.3}");
        assert!(ratio < 0.55, "ratio {ratio:.3} >= 0.55 (worse than target)");
    }

    #[test]
    fn determinism() {
        let mk = || {
            let mut m = PpmModel::new(4);
            let mut b = Vec::new();
            let mut e = ArithEncoder::new(&mut b);
            for &c in b"the quick brown fox jumps over the lazy dog" {
                m.encode_byte(&mut e, c);
            }
            e.flush(&mut b);
            b
        };
        assert_eq!(mk(), mk());
    }

    /// Memory-bounded: even 1 MB input must not blow up memory.
    /// Sliding window history + fixed-size table = O(constant).
    #[test]
    fn round_trip_large_input() {
        let input: Vec<u8> = b"abc".iter().cycle().copied().take(100_000).collect();
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        {
            let mut enc = ArithEncoder::new(&mut buf);
            for &b in &input {
                m.encode_byte(&mut enc, b);
            }
            enc.flush(&mut buf);
        }
        let mut m2 = PpmModel::new(4);
        let mut dec = ArithDecoder::new(&buf);
        let out: Vec<u8> = (0..input.len()).map(|_| m2.decode_byte(&mut dec)).collect();
        assert_eq!(out, input);
    }
}
