# 257 — LZMA BT4 Match Finder

- **Priority:** P1 (5-15% ratio gap at L9+)
- **Crate:** `omnizip-lzma`
- **Depends on:** [108](108-lzma-bt4-match-finder.md)
- **Estimated effort:** 5 days

## Problem

The current LZMA encoder uses a hash-chain match finder
(`HashChainMatchFinder` in omnizip-codecs). Hash chains find
matches in O(max_chain) per position with no guarantee of finding
the longest match.

The C reference (xz-utils) uses a binary tree (BT4) match finder
at higher quality levels. BT4 finds the longest match in
O(log N) per position by maintaining a binary search tree of all
previous positions.

Observed ratio gap to xz-utils L9: 5-15% on text data, larger on
highly repetitive data.

## Design

### BT4 algorithm

Binary tree of suffixes. Each node has:
- Position in input
- Left child (smaller suffix)
- Right child (larger suffix)

For each new position:
1. Walk the tree comparing bytes at each node.
2. Track longest match found.
3. Insert the new position, rebalancing as needed.

### Module structure

```
omnizip-lzma/src/encoder/
├── match_finder.rs        (existing hash chain wrapper)
├── bt4_match_finder.rs    (new)
└── optimal_encoder.rs     (uses BT4 at L7+)
```

### API parity with HashChainMatchFinder

The BT4 finder implements the same `find_match(pos) -> Option<Lz77Match>`
interface as the hash chain. This makes it a drop-in replacement
in the optimal encoder.

```rust
pub struct Bt4MatchFinder<'a> {
    data: &'a [u8],
    tree: Vec<Bt4Node>,     // 2 nodes per position (left, right)
    head: Vec<u32>,         // hash bucket roots
    // ... config
}

impl<'a> Bt4MatchFinder<'a> {
    pub fn new(data: &'a [u8], config: Bt4Config) -> Self { ... }
    pub fn advance(&mut self) -> Option<usize> { ... }
    pub fn find_match(&self, pos: usize) -> Option<Lz77Match> { ... }
    pub fn find_all_matches(&self, pos: usize) -> Vec<Lz77Match> { ... }
}
```

### Match finder trait

Both finders implement a common trait so the encoder can swap:

```rust
pub trait MatchFinder {
    fn advance(&mut self) -> Option<usize>;
    fn find_match(&self, pos: usize) -> Option<Lz77Match>;
    fn min_match(&self) -> u32;
}
```

Encoder takes `&mut dyn MatchFinder`.

### Quality → finder mapping

- L0-3: hash chain (fast modes)
- L4-6: hash chain with lazy2 (current)
- L7-9: BT4 (new) — finds longer matches

## Acceptance criteria

- [ ] BT4 match finder implemented and benchmarked.
- [ ] LZMA L7-9 uses BT4; L0-6 still uses hash chain.
- [ ] Ratio improvement on Calgary text corpus: 3-10% at L9.
- [ ] Speed regression at L9 < 2× (BT4 is slower but xz-utils
      achieves similar speed).
- [ ] All LZMA tests pass round-trip.
- [ ] Differential parity with `xz -d` maintained.

## Why this matters

LZMA's whole selling point is high ratio. A 5-15% gap to xz-utils
at max quality makes us "LZMA-like" rather than "LZMA". Closing
this gap is required to claim "real LZMA" in marketing.
