# 224 — ZSTD Binary Tree Match Finder

- **Priority:** P3 (ratio win at L16+)
- **Crate:** `omnizip-zstd`
- **Depends on:** [211](211-zstd-strategy-dispatch.md)
- **Estimated effort:** 3 days

## Goal

Implement a binary tree (BT) match finder for ZSTD levels 16+ (Btopt,
Btultra, Btultra2 strategies). The BT finds longer matches than the
hash-chain approach by maintaining a binary search tree of prior
positions, enabling O(log N) best-match lookup.

## Background

The C reference ZSTD encoder uses different match finders per strategy:
- Fast/DoubleFast: single-probe hash table (L1-L4)
- Greedy/Lazy/Lazy2: hash chain (L5-L12)
- Btlazy2/Btopt/Btultra/Btultra2: binary tree (L13+)

Our encoder uses hash-chain for all strategies ≥ Lazy. The BT provides
better match quality at the cost of more memory and computation.

## Current state

- L13-L22 use `compress_block_lazy2` (hash-chain based)
- The hash chain has `max_chain` entries walked per position
- BT would provide better candidates with the same memory budget

## Design

```rust
pub struct BtMatchFinder {
    // Binary tree: left_child[pos] and right_child[pos]
    left_child: Vec<u32>,
    right_child: Vec<u32>,
    hash_table: Vec<u32>,
    hash_log: u32,
}
```

The BT maintains, for each hash bucket, a binary search tree ordered by
lexicographic comparison of the 4-byte prefixes. This enables:
- O(log N) longest-match search
-自然 pruning of stale entries (beyond the window)

## Acceptance criteria

- [ ] BT match finder implemented for L16+ (Btopt strategy and above)
- [ ] BT finds matches >= hash-chain matches at the same position
- [ ] Memory usage <= 2 × hash-chain (two child arrays)
- [ ] Round-trip tests pass at L16-L22
- [ ] Ratio improvement >= 2% on text fixtures at L19+
