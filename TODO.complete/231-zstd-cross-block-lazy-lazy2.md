# 231 — ZSTD Cross-Block Matching for Lazy/Lazy2 (L6-L18)

- **Priority:** P2 (ratio win at mid-range levels on large inputs)
- **Crate:** `omnizip-zstd`
- **Depends on:** [208](208-zstd-cross-block-matching) (Fast/Greedy already
  cross-block; this extends to Lazy/Lazy2)
- **Estimated effort:** 2 days

## Goal

Enable cross-block hash table matching for Lazy/Lazy2 strategies (L6-L18).
Currently, these levels clear the hash table between blocks, limiting matches
to within a single 127 KiB block.

## Challenge

Lazy/Lazy2 use hash-chain walking for better match quality. The chain table
is sized for BLOCK_MAX_SIZE (128 KiB). With absolute positions across blocks,
the chain index would overflow.

## Solution: Ring Buffer Chain

Convert the chain table from direct-indexed to ring-buffer-indexed:

```rust
const CHAIN_BITS: u32 = 18;  // 256K entries
const CHAIN_SIZE: usize = 1 << CHAIN_BITS;
const CHAIN_MASK: usize = CHAIN_SIZE - 1;

// Chain access: chain[pos & CHAIN_MASK] instead of chain[pos]
```

The ring buffer naturally handles the 128 KiB distance window without
overflow. Collisions (two positions mapping to the same ring index) are
tolerated — they just cause missed matches, not incorrect output (match
verification always compares actual bytes).

## Plan

1. Add `CHAIN_BITS` constant and `chain_mask` to MatchState
2. Change all chain access from `chain[pos]` to `chain[pos & chain_mask]`
3. In `write_block`, don't clear the hash table for Lazy/Lazy2
4. Use `_with_prefix` match finders with absolute positions
5. Verify match verification still rejects invalid candidates

## Acceptance criteria

- [ ] Ring buffer chain implemented (256K entries, 1 MB memory)
- [ ] Lazy/Lazy2 strategies use cross-block matching
- [ ] No match quality regression (chain depth unchanged)
- [ ] Round-trip tests pass at L6-L18 on multi-block inputs
- [ ] Ratio improvement >= 1% on inputs > 256 KiB with repetition
