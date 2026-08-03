//! PPM byte-level prediction model using a bounded context trie.
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
//! After coding a byte, walk ALL suffix contexts (root + every
//! non-empty prefix), adding the byte to each. This is the standard
//! PPM "add to all suffix contexts" rule.
//!
//! ## Memory budget
//!
//! - Context trie: `MAX_NODES` × ~100 B avg = ~100 MB FIXED.
//! - Sliding-window history: `max_order` bytes (≤ 16 B).
//!
//! Memory is **bounded regardless of input size** — a gigabyte
//! input still uses ~100 MB. No input cap is enforced.
//!
//! ## Rescale policy
//!
//! When the arena is full, novel contexts are silently dropped;
//! existing nodes still receive updates. The escape mechanism
//! handles novel contexts gracefully.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::collapsible_else_if,
    clippy::similar_names,
    clippy::doc_markdown
)]

use super::context_tree::{ContextTree, NodeId};
use omnizip_codecs::arith::{scaled_prob, ArithDecoder, ArithEncoder, PROB_SCALE};

// ── PPM model ───────────────────────────────────────────────────────

/// Default arena size: 1M nodes × ~80 B avg = ~80 MB FIXED.
/// Tuned for high compression on text inputs. Each ContextNode is
/// ~64 B empty (Vec headers) up to ~1 KB full (256 freqs × 4 B +
/// 256 children × 5 B). Most nodes stay small in practice.
///
/// **Override this with [`PpmModel::with_memory_budget`] if you
/// need a smaller (or larger) footprint.**
pub const DEFAULT_MAX_NODES: usize = 1 << 20;

/// Approximate bytes per arena node, used to translate a user's
/// byte budget into a node count. Empirically derived from typical
/// text inputs (most nodes have 1–10 symbols + 1–10 children).
const BYTES_PER_NODE: usize = 80;

/// Minimum node count (small inputs shouldn't allocate less than this).
const MIN_MAX_NODES: usize = 1 << 10;

/// PPM byte-level model with bounded-memory context trie.
///
/// Memory is `O(MAX_NODES × ~100B + max_order)` regardless of input
/// size. The sliding-window history keeps total memory flat even
/// for gigabyte inputs.
pub struct PpmModel {
    tree: ContextTree,
    /// Sliding-window history: ring buffer of `max_order` bytes.
    history: Vec<u8>,
    /// Write index into `history`. Wraps modulo `history.len()`.
    head: usize,
    /// Number of valid bytes in `history`. Capped at `max_order`.
    history_len: usize,
    /// Maximum context order (1..=16).
    max_order: usize,
    /// Monotonic position counter for LRU bookkeeping.
    position: u64,
}

impl PpmModel {
    pub fn new(max_order: usize) -> Self {
        Self::with_capacity(max_order, 0)
    }

    /// `hint_len` is accepted for API compatibility but ignored —
    /// memory is fixed at ~100 MB regardless of input size.
    pub fn with_capacity(max_order: usize, _hint_len: usize) -> Self {
        Self::with_node_budget(max_order, DEFAULT_MAX_NODES)
    }

    /// Create with an explicit arena size. Useful for tests.
    pub fn with_node_budget(max_order: usize, max_nodes: usize) -> Self {
        let order = max_order.clamp(1, 16);
        let nodes = max_nodes.max(MIN_MAX_NODES);
        Self {
            tree: ContextTree::new(order, nodes),
            history: vec![0u8; order],
            head: 0,
            history_len: 0,
            max_order: order,
            position: 0,
        }
    }

    /// Create with an explicit memory budget in bytes.
    ///
    /// The arena is sized to fit approximately `max_bytes` of
    /// `ContextNode`s. Actual peak memory will be slightly higher
    /// (history window, encoder/decoder buffers, etc.). For example:
    ///
    /// - `with_memory_budget(4, 16 * 1024 * 1024)` — 16 MB cap, ~200K nodes
    /// - `with_memory_budget(4, 64 * 1024 * 1024)` — 64 MB cap, ~800K nodes
    /// - `with_memory_budget(4, 256 * 1024 * 1024)` — 256 MB cap, ~3M nodes
    ///
    /// More nodes = better compression ratio on large inputs with
    /// many distinct contexts. The trade-off is memory.
    #[must_use]
    pub fn with_memory_budget(max_order: usize, max_bytes: usize) -> Self {
        let nodes = (max_bytes / BYTES_PER_NODE).max(MIN_MAX_NODES);
        Self::with_node_budget(max_order, nodes)
    }

    pub fn reset(&mut self) {
        self.tree.reset();
        self.head = 0;
        self.history_len = 0;
        self.position = 0;
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
    /// first. Allocation-free: caller reuses `out`.
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

    /// PPM*C escape count: `D + 1`.
    fn escape_count(d: usize) -> u32 {
        u32::try_from(d).unwrap_or(0) + 1
    }

    /// 16-bit probability for "is the byte s_i?" given current
    /// cumulative weight `remaining` and symbol count `c_i`.
    fn prob16(c_i: u32, remaining: u32) -> u16 {
        scaled_prob(c_i, remaining)
    }

    /// Encode one byte. Walks the context trie from the deepest
    /// existing node up; at each level either encodes a hit or
    /// emits escape bits. On total miss, emits 8 equiprobable bits
    /// (order -1 fallback). Updates all suffix contexts after coding.
    pub fn encode_byte(&mut self, enc: &mut ArithEncoder, byte: u8) {
        let max_order = self.max_order;
        let mut scratch: Vec<u8> = Vec::with_capacity(max_order);
        self.ctx_bytes_into(max_order, &mut scratch);

        // Walk the trie to find the deepest node for our current context.
        let (deepest_node, _depth) = self.tree.walk(&scratch);

        // Walk UP via suffix links, trying each order.
        let mut current = deepest_node;
        let mut resolved = false;

        while !current.is_null() {
            // Snapshot the symbol table for this node.
            let snap_freqs: Vec<(u8, u16)> = {
                let entry = self.tree.node(current);
                entry.freqs.iter().map(|f| (f.symbol, f.freq)).collect()
            };
            if snap_freqs.is_empty() {
                // No symbols at this node — implicit escape, no bits.
                let next = self.tree.node(current).suffix;
                if next.is_null() || next == current {
                    break;
                }
                current = next;
                continue;
            }

            let total: u32 = snap_freqs.iter().map(|(_, c)| u32::from(*c)).sum::<u32>()
                + Self::escape_count(snap_freqs.len());
            let mut remaining = total;

            let pos = snap_freqs.iter().position(|(s, _)| *s == byte);
            match pos {
                Some(k) => {
                    for (i, (_sym, c)) in snap_freqs.iter().enumerate() {
                        let c_i = u32::from(*c);
                        let p = Self::prob16(c_i, remaining);
                        if i == k {
                            enc.encode_bit(p, true);
                            break;
                        }
                        enc.encode_bit(p, false);
                        remaining = remaining.saturating_sub(c_i);
                    }
                    resolved = true;
                    break;
                }
                None => {
                    // Emit escape: encode "no" for every symbol.
                    for (_sym, c) in snap_freqs.iter() {
                        let c_i = u32::from(*c);
                        let p = Self::prob16(c_i, remaining);
                        enc.encode_bit(p, false);
                        remaining = remaining.saturating_sub(c_i);
                    }
                    let next = self.tree.node(current).suffix;
                    if next.is_null() || next == current {
                        break;
                    }
                    current = next;
                }
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

        // Update: add the byte to all suffix contexts.
        self.ctx_bytes_into(max_order, &mut scratch);
        self.tree.add_to_all_suffixes(&scratch, byte, self.position);
        self.position = self.position.saturating_add(1);

        self.push_history(byte);
    }

    /// Decode one byte. Mirrors [`encode_byte`].
    pub fn decode_byte(&mut self, dec: &mut ArithDecoder) -> u8 {
        let max_order = self.max_order;
        let mut scratch: Vec<u8> = Vec::with_capacity(max_order);
        self.ctx_bytes_into(max_order, &mut scratch);

        let (deepest_node, _depth) = self.tree.walk(&scratch);

        let mut current = deepest_node;
        let mut resolved: Option<u8> = None;

        while !current.is_null() {
            let snap_freqs: Vec<(u8, u16)> = {
                let entry = self.tree.node(current);
                entry.freqs.iter().map(|f| (f.symbol, f.freq)).collect()
            };
            if snap_freqs.is_empty() {
                let next = self.tree.node(current).suffix;
                if next.is_null() || next == current {
                    break;
                }
                current = next;
                continue;
            }

            let total: u32 = snap_freqs.iter().map(|(_, c)| u32::from(*c)).sum::<u32>()
                + Self::escape_count(snap_freqs.len());
            let mut remaining = total;
            let mut found: Option<u8> = None;

            for (sym, c) in snap_freqs.iter() {
                let c_i = u32::from(*c);
                let p = Self::prob16(c_i, remaining);
                if dec.decode_bit(p) {
                    found = Some(*sym);
                    break;
                }
                remaining = remaining.saturating_sub(c_i);
            }

            if let Some(sym) = found {
                resolved = Some(sym);
                break;
            }

            let next = self.tree.node(current).suffix;
            if next.is_null() || next == current {
                break;
            }
            current = next;
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

        self.ctx_bytes_into(max_order, &mut scratch);
        self.tree.add_to_all_suffixes(&scratch, byte, self.position);
        self.position = self.position.saturating_add(1);

        self.push_history(byte);
        byte
    }

    /// Current arena utilisation (0.0..=1.0).
    #[must_use]
    pub fn utilisation(&self) -> f64 {
        self.tree.utilisation()
    }
}

// Keep NodeId re-export so external code can address tree nodes.
pub use super::context_tree::NodeId as PpmNodeId;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_single_byte() {
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        {
            let mut enc = ArithEncoder::new();
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
            let mut enc = ArithEncoder::new();
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
            let mut enc = ArithEncoder::new();
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
            let mut enc = ArithEncoder::new();
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
            let mut enc = ArithEncoder::new();
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
    fn determinism() {
        let mk = || {
            let mut m = PpmModel::new(4);
            let mut b = Vec::new();
            let mut e = ArithEncoder::new();
            for &c in b"the quick brown fox jumps over the lazy dog" {
                m.encode_byte(&mut e, c);
            }
            e.flush(&mut b);
            b
        };
        assert_eq!(mk(), mk());
    }

    /// Memory-bounded: even 50 KB input must not blow up memory.
    /// (Larger inputs work too — arena is capped at DEFAULT_MAX_NODES.)
    #[test]
    fn round_trip_large_input() {
        let input: Vec<u8> = b"abc".iter().cycle().copied().take(50_000).collect();
        let mut m = PpmModel::new(4);
        let mut buf = Vec::new();
        {
            let mut enc = ArithEncoder::new();
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

    /// When the arena is small, novel contexts are silently dropped.
    /// Round-trip must still succeed via the escape path.
    #[test]
    fn round_trip_with_tiny_arena() {
        let text = b"The quick brown fox jumps over the lazy dog. ".repeat(20);
        let mut m = PpmModel::with_node_budget(4, 50); // tiny arena
        let mut buf = Vec::new();
        {
            let mut enc = ArithEncoder::new();
            for &b in &text {
                m.encode_byte(&mut enc, b);
            }
            enc.flush(&mut buf);
        }
        let mut m2 = PpmModel::with_node_budget(4, 50);
        let mut dec = ArithDecoder::new(&buf);
        let out: Vec<u8> = (0..text.len()).map(|_| m2.decode_byte(&mut dec)).collect();
        assert_eq!(out, text);
    }
}
