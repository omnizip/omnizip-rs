//! PPM model orchestrator.
//!
//! Drives the [`crate::context_tree::ContextTree`] and the
//! [`crate::range_coder`] together. The model is the single source of
//! truth for "what context am I in, and how do I code this byte?" —
//! encoder and decoder run identical model logic so the trees stay in
//! sync.
//!
//! ## Coding a byte (PPM with escape, PPM*C-ish)
//!
//! For each input byte `s` with history `h`:
//!
//! 1. Start at the deepest available context (up to `max_order` bytes
//!    of `h`). Walk the trie down.
//! 2. At each context node, query `lookup(s)`:
//!    - If `s` is present: encode its `[cum_lo, cum_lo + freq)` slot
//!      (scaled by `coding_total`), then **stop**.
//!    - If `s` is absent: encode the escape slot, drop one byte of
//!      context, and try again.
//! 3. If we reach the root and `s` is still absent: encode the escape
//!    at root, then encode `s` against the uniform order-(-1) model
//!    (256-way).
//! 4. After coding `s`, call `tree.add_symbol(history, s)` to update
//!    every suffix context. This is the only mutation; both sides do
//!    it identically.
//!
//! ## Decoder mirror
//!
//! The decoder does the same walk, but at each node it asks the range
//! decoder "which slot is the next symbol in?" and compares against
//! the symbol table to recover `s`.

#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_lossless)]

use crate::context_tree::ContextTree;
use crate::range_coder::{RangeDecoder, RangeEncoder};

/// Uniform distribution total for the order-(-1) fallback.
const UNIFORM_TOTAL: u32 = 256;

/// The PPM model. Owns the context tree and tracks history.
#[derive(Debug)]
pub struct PpmModel {
    tree: ContextTree,
    /// Sliding window of recent bytes (most-recent last). Bounded to
    /// `max_order` bytes for context lookup, but we keep the full
    /// history here for `add_symbol` to update all suffix contexts.
    history: Vec<u8>,
    max_order: usize,
}

impl PpmModel {
    /// Create a fresh model with the given max context order.
    #[must_use]
    pub fn new(max_order: usize) -> Self {
        Self {
            tree: ContextTree::new(max_order),
            history: Vec::new(),
            max_order,
        }
    }

    /// Encode one byte `s` given the current history.
    pub fn encode_byte(&mut self, enc: &mut RangeEncoder<'_>, s: u8) {
        // The context for this byte is the last `max_order` bytes of history.
        let ctx_window: &[u8] = if self.history.len() >= self.max_order {
            &self.history[self.history.len() - self.max_order..]
        } else {
            &self.history[..]
        };

        // Walk from deepest context to shallowest. At each level we have
        // a node and a context slice of that depth.
        let mut emitted = false;
        for depth in (1..=ctx_window.len()).rev() {
            let ctx_slice = &ctx_window[ctx_window.len() - depth..];
            let (node, _reached) = self.tree.walk(ctx_slice);
            if let Some((lo, freq, _total)) = node.lookup(s) {
                let total = node.coding_total();
                enc.encode(lo, lo + freq, total);
                emitted = true;
                break;
            }
            // Escape: emit the escape slot.
            let (esc_lo, esc_width) = node.escape_slot();
            let total = node.coding_total();
            enc.encode(esc_lo, esc_lo + esc_width, total);
        }

        if !emitted {
            // Order 0: the root node (empty context).
            let root = self.tree.root();
            if let Some((lo, freq, _total)) = root.lookup(s) {
                let total = root.coding_total();
                enc.encode(lo, lo + freq, total);
                emitted = true;
            } else {
                let (esc_lo, esc_width) = root.escape_slot();
                let total = root.coding_total();
                enc.encode(esc_lo, esc_lo + esc_width, total);
            }
        }

        if !emitted {
            // Order -1: uniform.
            let sym_lo = u32::from(s);
            enc.encode(sym_lo, sym_lo + 1, UNIFORM_TOTAL);
        }

        // Update model: record `s` in all suffix contexts.
        self.tree.add_symbol(&self.history, s);
        self.history.push(s);
    }

    /// Decode one byte given the current history. Mirrors
    /// [`Self::encode_byte`] exactly.
    pub fn decode_byte(&mut self, dec: &mut RangeDecoder<'_>) -> u8 {
        let ctx_window: &[u8] = if self.history.len() >= self.max_order {
            &self.history[self.history.len() - self.max_order..]
        } else {
            &self.history[..]
        };

        // Walk deepest to shallowest.
        for depth in (1..=ctx_window.len()).rev() {
            let ctx_slice = &ctx_window[ctx_window.len() - depth..];
            let (node, _reached) = self.tree.walk(ctx_slice);
            let total = node.coding_total();
            let target = dec.target_freq(total);
            let (esc_lo, esc_width) = node.escape_slot();
            if target < esc_lo {
                // The symbol is one of the entries at this node.
                let (sym, lo, freq) = node.find_symbol_at_freq(target);
                dec.advance(lo, lo + freq, total);
                self.commit(sym);
                return sym;
            }
            // Escape: advance past the escape slot, drop to shorter context.
            dec.advance(esc_lo, esc_lo + esc_width, total);
        }

        // Order 0: root.
        let total = self.tree.root().coding_total();
        let target = dec.target_freq(total);
        let (esc_lo, esc_width) = self.tree.root().escape_slot();
        if target < esc_lo {
            let (sym, lo, freq) = self.tree.root().find_symbol_at_freq(target);
            dec.advance(lo, lo + freq, total);
            self.commit(sym);
            return sym;
        }
        dec.advance(esc_lo, esc_lo + esc_width, total);

        // Order -1: uniform.
        let target = dec.target_freq(UNIFORM_TOTAL);
        let sym = target.min(255) as u8;
        dec.advance(u32::from(sym), u32::from(sym) + 1, UNIFORM_TOTAL);
        self.commit(sym);
        sym
    }

    /// Record the decoded/encoded symbol in the model. Shared by both paths.
    fn commit(&mut self, s: u8) {
        self.tree.add_symbol(&self.history, s);
        self.history.push(s);
    }
}
