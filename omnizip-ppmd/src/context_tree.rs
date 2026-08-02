//! PPM context trie.
//!
//! A trie of suffix contexts. Each node represents one byte of history;
//! the path from root to a node is a context string (most-recent-byte
//! deepest). At each node we keep a frequency table of symbols observed
//! *after* that context.
//!
//! ## Determinism
//!
//! Frequency tables use a fixed insertion order: symbols are stored in a
//! `Vec<(symbol, freq)>` sorted by first-occurrence. Frequencies are
//! incremented by 1 per occurrence. The escape count is the number of
//! distinct symbols at the node. This makes cumulative-frequency
//! computation deterministic across runs and across encoder/decoder.
//!
//! ## Memory
//!
//! For Phase 1 we cap the trie depth at `max_order` (default 4) and do
//! not bound total node count — Phase 1 inputs are small enough that
//! this is fine. A bounded allocator is Phase 2 work.

#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_lossless)]

/// A symbol's frequency entry in a context node.
#[derive(Clone, Copy, Debug)]
struct FreqEntry {
    symbol: u8,
    freq: u32,
}

/// A node in the context trie.
#[derive(Clone, Debug, Default)]
pub struct ContextNode {
    /// Symbols observed after this context, in first-occurrence order.
    /// We do NOT sort by frequency — order matters for cumulative freq
    /// computation and must be deterministic.
    freqs: Vec<FreqEntry>,
    /// Total of all frequencies (cached for speed).
    total: u32,
    /// Child contexts, keyed by the byte that extends this context.
    /// Stored as a flat Vec; lookup is linear since fanout is bounded
    /// by alphabet size and depth by max_order.
    children: Vec<(u8, ContextNode)>,
}

impl ContextNode {
    /// Find the frequency slot for `symbol`, returning
    /// `(cum_freq_lo, sym_freq, total)` if present.
    ///
    /// * `cum_freq_lo` = sum of frequencies of symbols before this one.
    /// * `sym_freq` = this symbol's frequency.
    /// * `total` = sum of all frequencies at this node (including escape).
    #[must_use]
    pub fn lookup(&self, symbol: u8) -> Option<(u32, u32, u32)> {
        let mut cum = 0u32;
        for e in &self.freqs {
            if e.symbol == symbol {
                return Some((cum, e.freq, self.total));
            }
            cum += e.freq;
        }
        None
    }

    /// Cumulative-frequency slot for the escape symbol: `[total, total + num_distinct)`.
    /// The escape "frequency" is the number of distinct symbols seen at
    /// this node (PPM*C-ish escape — see Shkarin / Witten-Bell variants).
    /// We add 1 to ensure non-zero probability.
    #[must_use]
    pub fn escape_slot(&self) -> (u32, u32) {
        let lo = self.total;
        let width = self.freqs.len() as u32 + 1;
        (lo, width)
    }

    /// Total used for arithmetic coding: sum of symbol freqs + escape width.
    #[must_use]
    pub fn coding_total(&self) -> u32 {
        self.total + self.freqs.len() as u32 + 1
    }

    /// Record that `symbol` followed this context. Updates the frequency
    /// table and total. Idempotent on the symbol count (each call is
    /// one observation).
    pub fn add_symbol(&mut self, symbol: u8) {
        if let Some(e) = self.freqs.iter_mut().find(|e| e.symbol == symbol) {
            e.freq += 1;
        } else {
            self.freqs.push(FreqEntry { symbol, freq: 1 });
        }
        self.total += 1;
    }

    /// Find the symbol whose cumulative-frequency range contains `target`,
    /// returning `(symbol, cum_lo, sym_freq)`. The caller must guarantee
    /// `target < self.total` (i.e. `target` is inside the symbol region,
    /// not the escape slot).
    ///
    /// # Panics
    ///
    /// Debug builds assert that a matching entry is found.
    #[must_use]
    pub fn find_symbol_at_freq(&self, target: u32) -> (u8, u32, u32) {
        debug_assert!(target < self.total, "target must be inside symbol region");
        let mut cum = 0u32;
        for e in &self.freqs {
            if target < cum + e.freq {
                return (e.symbol, cum, e.freq);
            }
            cum += e.freq;
        }
        // `target < total` guarantees we always find an entry; this is a
        // safety net for corrupt state.
        let last = self
            .freqs
            .last()
            .expect("non-empty freq table when total > 0");
        let cum = self.total - last.freq;
        (last.symbol, cum, last.freq)
    }

    /// Number of distinct symbols observed at this context.
    #[must_use]
    pub fn num_symbols(&self) -> usize {
        self.freqs.len()
    }

    /// Walk to (or create) the child for `byte`.
    pub fn child_mut(&mut self, byte: u8) -> &mut ContextNode {
        // Linear search — fanout is bounded.
        let idx = self.children.iter().position(|(b, _)| *b == byte);
        if let Some(i) = idx {
            &mut self.children[i].1
        } else {
            self.children.push((byte, ContextNode::default()));
            &mut self.children.last_mut().expect("just pushed").1
        }
    }

    /// Look up an existing child without creating one.
    pub fn child(&self, byte: u8) -> Option<&ContextNode> {
        self.children
            .iter()
            .find_map(|(b, n)| if *b == byte { Some(n) } else { None })
    }
}

/// The PPM context trie, rooted at the order-(-1) "uniform" model.
#[derive(Clone, Debug, Default)]
pub struct ContextTree {
    root: ContextNode,
    /// Maximum context order (trie depth beyond which we don't extend).
    max_order: usize,
}

impl ContextTree {
    /// Create a fresh tree with the given max order.
    #[must_use]
    pub fn new(max_order: usize) -> Self {
        Self {
            root: ContextNode::default(),
            max_order,
        }
    }

    /// The root node — corresponds to the empty context (order 0).
    #[must_use]
    pub fn root(&self) -> &ContextNode {
        &self.root
    }

    /// Mutable root.
    pub fn root_mut(&mut self) -> &mut ContextNode {
        &mut self.root
    }

    /// Walk down the context path `ctx` (most-recent-byte last),
    /// returning the deepest existing node along that path. Used to
    /// find the starting context for a prediction lookup.
    ///
    /// Returns `(node_ref, effective_depth)` where `effective_depth` is
    /// how deep we actually got (may be less than `ctx.len()` if the
    /// trie hasn't grown those branches yet).
    #[must_use]
    pub fn walk(&self, ctx: &[u8]) -> (&ContextNode, usize) {
        let mut node = &self.root;
        let mut depth = 0;
        for &b in ctx {
            match node.child(b) {
                Some(child) => {
                    node = child;
                    depth += 1;
                }
                None => break,
            }
        }
        (node, depth)
    }

    /// Update the tree after observing `symbol` following context `ctx`.
    ///
    /// For each suffix of `ctx` (longest first), up to `max_order` deep,
    /// record the symbol. This is the standard PPM "add to all suffix
    /// contexts" update — equivalent to a suffix-tree insertion.
    ///
    /// `ctx` is the full available history (most-recent-byte last). We
    /// take the last `max_order` bytes as the deepest context and walk
    /// down, creating nodes as needed.
    pub fn add_symbol(&mut self, ctx: &[u8], symbol: u8) {
        let depth = ctx.len().min(self.max_order);
        if depth == 0 {
            self.root.add_symbol(symbol);
            return;
        }

        // Walk down from root, creating nodes, recording symbol at each.
        let mut node: &mut ContextNode = &mut self.root;
        node.add_symbol(symbol);
        for &b in &ctx[ctx.len() - depth..] {
            node = node.child_mut(b);
            node.add_symbol(symbol);
        }
    }

    /// Max context order.
    #[must_use]
    pub fn max_order(&self) -> usize {
        self.max_order
    }
}
