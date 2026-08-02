//! Bounded PPM context trie with arena allocation.
//!
//! Each node represents one byte of context history. The path from
//! root to a node is a context string (most-recent byte deepest). At
//! each node we keep a frequency table of symbols observed AFTER
//! that context.
//!
//! ## Memory bounds
//!
//! Nodes live in a fixed-size arena (`Vec<ContextNode>`). The cap is
//! set at construction time and never grows. When the arena fills,
//! new contexts are NOT created — the encoder uses the escape path
//! instead. This guarantees bounded memory regardless of input size.
//!
//! Per node:
//! - `freqs`: up to 256 × 4 bytes = 1 KB
//! - `children`: up to 256 × 4 bytes (index) = 1 KB
//! - `total`, `glue_count`, `last_used`: 12 bytes
//! - Average real-world node: ~50-100 bytes
//!
//! For 1M nodes × 100 bytes = ~100 MB worst case (default cap).
//!
//! ## Determinism
//!
//! Frequency tables use first-occurrence insertion order: symbols
//! are appended to `freqs` when first observed at this context.
//! Frequencies are incremented by 1 per occurrence. The escape count
//! is the number of distinct symbols at the node + 1 (PPM*C).

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::doc_markdown
)]

/// Index into the arena. `0` is reserved for "null/empty"; real
/// nodes start at index 1 (the root).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Default)]
pub struct NodeId(pub u32);

impl NodeId {
    /// Sentinel for "no node".
    pub const NULL: NodeId = NodeId(0);
    /// The root node id.
    pub const ROOT: NodeId = NodeId(1);
    #[must_use]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }
}

/// A single (symbol, frequency) entry inside a context node.
#[derive(Clone, Copy, Debug)]
pub struct FreqEntry {
    pub symbol: u8,
    pub freq: u16,
}

/// A node in the context trie.
#[derive(Clone, Debug)]
pub struct ContextNode {
    /// Symbols observed after this context, in first-occurrence order.
    pub freqs: Vec<FreqEntry>,
    /// Sum of all frequencies (cached for speed).
    pub total: u32,
    /// Sparse child list: `(byte, child_id)` pairs, first-occurrence
    /// order. Stored as a Vec rather than `[NodeId; 256]` to keep
    /// empty nodes small (24 B vs 1 KB). Lookup is linear; fanout
    /// is bounded by 256 and typical is much smaller.
    pub children: Vec<(u8, NodeId)>,
    /// Suffix link: the node for this node's context minus its oldest
    /// byte. Used for O(1) order-(K-1) lookup after an order-K escape.
    pub suffix: NodeId,
    /// Last input position at which this node was used as a prediction
    /// source. Used for LRU-style pruning (Phase 2).
    pub last_used: u64,
}

impl Default for ContextNode {
    fn default() -> Self {
        Self {
            freqs: Vec::new(),
            total: 0,
            children: Vec::new(),
            suffix: NodeId::NULL,
            last_used: 0,
        }
    }
}

impl ContextNode {
    /// Look up `symbol` in this node's frequency table.
    /// Returns `(index, freq)` or `None`.
    #[must_use]
    pub fn find_symbol(&self, symbol: u8) -> Option<(usize, u16)> {
        self.freqs.iter().enumerate().find_map(|(i, e)| {
            if e.symbol == symbol {
                Some((i, e.freq))
            } else {
                None
            }
        })
    }

    /// Look up a child by byte.
    #[must_use]
    pub fn child(&self, byte: u8) -> NodeId {
        for (b, id) in &self.children {
            if *b == byte {
                return *id;
            }
        }
        NodeId::NULL
    }

    /// Add `symbol` to this node's frequency table (first occurrence)
    /// or increment its frequency. Idempotent on the symbol count.
    pub fn add_observation(&mut self, symbol: u8) {
        if let Some(e) = self.freqs.iter_mut().find(|e| e.symbol == symbol) {
            if e.freq < u16::MAX {
                e.freq += 1;
            }
        } else if self.freqs.len() < 256 {
            self.freqs.push(FreqEntry { symbol, freq: 1 });
        }
        self.total = self.total.saturating_add(1);
    }

    /// Number of distinct symbols observed at this context.
    #[must_use]
    pub fn num_symbols(&self) -> usize {
        self.freqs.len()
    }

    /// Halve all frequencies (floor at 1). Used during rescale to
    /// free statistical weight when an entry's count saturates.
    pub fn rescale(&mut self) {
        let mut new_total: u32 = 0;
        for e in self.freqs.iter_mut() {
            let new_f = (u32::from(e.freq) / 2).max(1) as u16;
            e.freq = new_f;
            new_total += u32::from(new_f);
        }
        self.total = new_total;
    }
}

/// Bounded arena-allocated context trie.
///
/// Memory is fixed at construction time. The arena never grows.
/// When full, new contexts are not created; existing contexts still
/// receive symbol updates.
pub struct ContextTree {
    /// The arena. Index 0 is NULL (unused); index 1 is ROOT; the
    /// rest are dynamically allocated contexts.
    nodes: Vec<ContextNode>,
    /// Maximum number of nodes (including root). Sets the memory cap.
    max_nodes: usize,
    /// Maximum context order (trie depth beyond which we don't extend).
    max_order: usize,
    /// Next free slot in the arena. Equals `nodes.len()` when full.
    next_free: usize,
}

impl ContextTree {
    /// Create a fresh tree with the given capacity and max order.
    ///
    /// `max_nodes` is the hard cap on arena size. ~1M nodes × 100 B
    /// avg = ~100 MB. Smaller budgets trade ratio for memory.
    #[must_use]
    pub fn new(max_order: usize, max_nodes: usize) -> Self {
        let mut nodes = Vec::with_capacity(max_nodes);
        // Slot 0: NULL sentinel (unused but reserved).
        nodes.push(ContextNode::default());
        // Slot 1: ROOT (the order-0 context — empty context).
        nodes.push(ContextNode::default());
        Self {
            nodes,
            max_nodes,
            max_order: max_order.clamp(1, 16),
            next_free: 2,
        }
    }

    /// Maximum context order.
    #[must_use]
    pub fn max_order(&self) -> usize {
        self.max_order
    }

    /// Current arena utilisation (allocated nodes / capacity).
    #[must_use]
    pub fn utilisation(&self) -> f64 {
        self.next_free as f64 / self.max_nodes as f64
    }

    /// Borrow a node by id. Returns the NULL sentinel's empty node
    /// if id is null (defensive).
    #[must_use]
    pub fn node(&self, id: NodeId) -> &ContextNode {
        let idx = if id.is_null() { 0 } else { id.0 as usize };
        &self.nodes[idx]
    }

    /// Mutably borrow a node by id.
    pub fn node_mut(&mut self, id: NodeId) -> &mut ContextNode {
        let idx = if id.is_null() { 0 } else { id.0 as usize };
        &mut self.nodes[idx]
    }

    /// Allocate a fresh node. Returns `NULL` if the arena is full
    /// (caller should fall back to escape path).
    fn alloc(&mut self) -> NodeId {
        if self.next_free >= self.max_nodes {
            return NodeId::NULL;
        }
        let id = NodeId(self.next_free as u32);
        // Ensure the Vec has capacity (it grew to max_nodes at construction).
        if self.nodes.len() <= self.next_free {
            self.nodes.push(ContextNode::default());
        }
        self.next_free += 1;
        id
    }

    /// Find or create the child of `parent` for `byte`. Returns NULL
    /// if the arena is full and no new node can be allocated.
    fn child_mut(&mut self, parent: NodeId, byte: u8) -> NodeId {
        let existing = self.node(parent).child(byte);
        if !existing.is_null() {
            return existing;
        }
        let new_id = self.alloc();
        if new_id.is_null() {
            return NodeId::NULL;
        }
        // Set suffix link: child's suffix is parent's suffix + byte
        // (or ROOT if parent is ROOT). For simplicity, we point all
        // children's suffix to ROOT — proper suffix links require a
        // more complex construction (Aho-Corasick style) which is
        // future work. ROOT fallback gives correct order-(-1) escape.
        let mut new_suffix = if parent == NodeId::ROOT {
            NodeId::ROOT
        } else {
            // Walk parent's suffix chain looking for a child on `byte`.
            let mut cur = self.node(parent).suffix;
            let mut found = NodeId::ROOT;
            while !cur.is_null() && cur != NodeId::ROOT {
                let candidate = self.node(cur).child(byte);
                if !candidate.is_null() {
                    found = candidate;
                    break;
                }
                cur = self.node(cur).suffix;
            }
            found
        };
        if new_suffix.is_null() {
            new_suffix = NodeId::ROOT;
        }
        self.node_mut(new_id).suffix = new_suffix;
        // Append to parent's children list (sparse).
        self.node_mut(parent).children.push((byte, new_id));
        new_id
    }

    /// Walk down the context path `ctx` (oldest first), returning the
    /// deepest existing node along that path. Used to find the
    /// starting context for a prediction lookup.
    ///
    /// Returns `(node_id, effective_depth)` where effective_depth is
    /// how deep we actually got (may be less than ctx.len() if the
    /// trie hasn't grown those branches yet).
    #[must_use]
    pub fn walk(&self, ctx: &[u8]) -> (NodeId, usize) {
        let mut node = NodeId::ROOT;
        let mut depth = 0;
        for &b in ctx {
            let next = self.node(node).child(b);
            if next.is_null() {
                break;
            }
            node = next;
            depth += 1;
        }
        (node, depth)
    }

    /// Update the tree after observing `symbol` following context `ctx`.
    ///
    /// For each prefix of `ctx` (longest first), up to `max_order` deep,
    /// record the symbol at that node. Creates nodes as needed; if the
    /// arena is full, silently drops the observation (the existing
    /// probability distribution is still used for prediction).
    ///
    /// `ctx` is the context bytes (oldest first, most-recent last).
    pub fn add_observation(&mut self, ctx: &[u8], symbol: u8, position: u64) {
        let depth = ctx.len().min(self.max_order);
        if depth == 0 {
            let root = self.node_mut(NodeId::ROOT);
            root.add_observation(symbol);
            root.last_used = position;
            return;
        }
        // Walk down from root, creating nodes, recording symbol at each.
        let mut node: NodeId = NodeId::ROOT;
        self.node_mut(node).add_observation(symbol);
        self.node_mut(node).last_used = position;
        let start = ctx.len().saturating_sub(depth);
        for &b in &ctx[start..] {
            node = self.child_mut(node, b);
            if node.is_null() {
                return; // arena full
            }
            let n = self.node_mut(node);
            n.add_observation(symbol);
            n.last_used = position;
        }
    }

    /// Walk down `ctx` (creating nodes as needed), recording `symbol`
    /// at every prefix of the path (root + each non-empty prefix).
    /// This is the "add to all suffix contexts" PPM update rule.
    pub fn add_to_all_suffixes(&mut self, ctx: &[u8], symbol: u8, position: u64) {
        let depth = ctx.len().min(self.max_order);

        // Root context always gets the symbol.
        let root = self.node_mut(NodeId::ROOT);
        root.add_observation(symbol);
        root.last_used = position;

        if depth == 0 {
            return;
        }

        // For each non-empty suffix of ctx[..depth], walk down and
        // record. We walk down once along the deepest path, recording
        // at every node along the way.
        let start = ctx.len().saturating_sub(depth);
        let mut node: NodeId = NodeId::ROOT;
        for &b in &ctx[start..] {
            node = self.child_mut(node, b);
            if node.is_null() {
                return;
            }
            let n = self.node_mut(node);
            n.add_observation(symbol);
            n.last_used = position;
        }
    }

    /// Halve every node's symbol frequencies. Used to free
    /// statistical weight when counts saturate.
    pub fn rescale_all(&mut self) {
        for node in self.nodes.iter_mut() {
            node.rescale();
        }
    }

    /// Reset to the empty state. Keeps the arena allocation.
    pub fn reset(&mut self) {
        for node in self.nodes.iter_mut() {
            *node = ContextNode::default();
        }
        self.next_free = 2; // NULL + ROOT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_allocates_sequentially() {
        let mut tree = ContextTree::new(4, 100);
        assert_eq!(tree.next_free, 2);
        let id = tree.alloc();
        assert_eq!(id, NodeId(2));
        assert_eq!(tree.next_free, 3);
    }

    #[test]
    fn arena_returns_null_when_full() {
        let mut tree = ContextTree::new(4, 3); // NULL + ROOT + 1 free
        let id = tree.alloc();
        assert!(!id.is_null());
        let full = tree.alloc();
        assert!(full.is_null());
    }

    #[test]
    fn walk_finds_existing_path() {
        let mut tree = ContextTree::new(4, 100);
        let ctx = b"abc";
        tree.add_observation(ctx, b'X', 0);
        let (node, depth) = tree.walk(ctx);
        assert_eq!(depth, 3);
        assert!(!node.is_null());
        let found = tree.node(node).find_symbol(b'X');
        assert!(found.is_some());
    }

    #[test]
    fn walk_stops_at_missing_branch() {
        let mut tree = ContextTree::new(4, 100);
        tree.add_observation(b"ab", b'X', 0);
        // Walking "ax" stops at depth 1 because 'x' child of root
        // doesn't exist. We end up at the 'a' node (which DOES have
        // X in its freqs because add_observation walks all prefixes).
        let (node, depth) = tree.walk(b"ax");
        assert_eq!(depth, 1);
        assert!(!node.is_null());
        // X is in 'a's freqs (add_observation visits every prefix).
        assert!(tree.node(node).find_symbol(b'X').is_some());
    }

    #[test]
    fn observation_increments_existing_symbol() {
        let mut tree = ContextTree::new(4, 100);
        tree.add_observation(b"ab", b'X', 0);
        tree.add_observation(b"ab", b'X', 0);
        tree.add_observation(b"ab", b'X', 0);
        let (node, _) = tree.walk(b"ab");
        let (_, freq) = tree.node(node).find_symbol(b'X').unwrap();
        assert_eq!(freq, 3);
    }

    #[test]
    fn add_to_all_suffixes_updates_root_and_chain() {
        let mut tree = ContextTree::new(4, 100);
        tree.add_to_all_suffixes(b"ab", b'X', 0);
        // Root context: should have X with freq 1.
        let root_freq = tree.node(NodeId::ROOT).find_symbol(b'X').unwrap().1;
        assert_eq!(root_freq, 1);
        // 'a' context: should have X with freq 1.
        let (a_node, _) = tree.walk(b"a");
        let a_freq = tree.node(a_node).find_symbol(b'X').unwrap().1;
        assert_eq!(a_freq, 1);
        // 'ab' context: should have X with freq 1.
        let (ab_node, _) = tree.walk(b"ab");
        let ab_freq = tree.node(ab_node).find_symbol(b'X').unwrap().1;
        assert_eq!(ab_freq, 1);
    }

    #[test]
    fn bounded_memory_never_exceeds_max_nodes() {
        let mut tree = ContextTree::new(4, 50);
        // Hammer with many distinct contexts — should not exceed 50 nodes.
        for i in 0..1000u32 {
            let ctx: Vec<u8> = (i.to_le_bytes()).to_vec();
            tree.add_observation(&ctx, b'X', u64::from(i));
        }
        assert!(tree.next_free <= tree.max_nodes);
    }

    #[test]
    fn rescale_halves_frequencies() {
        let mut tree = ContextTree::new(4, 100);
        // Build up a node with high counts.
        for _ in 0..10 {
            tree.add_observation(b"a", b'X', 0);
        }
        let (node, _) = tree.walk(b"a");
        let before = tree.node(node).find_symbol(b'X').unwrap().1;
        assert_eq!(before, 10);
        tree.rescale_all();
        let after = tree.node(node).find_symbol(b'X').unwrap().1;
        assert_eq!(after, 5);
    }
}
