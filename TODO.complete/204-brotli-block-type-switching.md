# 204 — Brotli Block Type Switching (NBLTYPES > 1)

- **Priority:** P3 (3% ratio win, moderate complexity)
- **Crate:** `omnizip-brotli`
- **Depends on:** [200](200-brotli-context-modeling.md) (context maps
  benefit from block switches)
- **Estimated effort:** 1 week

## Goal

Implement block type switching within a metablock. Currently
NBLTYPES=1 for all categories (literal/insert-copy/distance), meaning
a single Huffman table per category spans the entire metablock. Block
switching allows different tables for different regions of the input,
improving entropy adaptation.

## Background

RFC 7932 §9.3: each metablock can have up to 256 block types per
category. Block-switch commands (emitted inline in the command
stream) switch to a different block type, which selects a different
Huffman table from the tree group.

The reference encoder splits metablocks into ~6 KiB blocks by default,
each with its own optimal tables.

## Scope

1. **Block boundaries** (2 days): decide where to split the metablock
   into blocks. Heuristic: fixed-size blocks (default 6 KiB) or
   ratio-feedback (split when current tables stop fitting).

2. **Per-block tables** (3 days): build separate Huffman tables for
   each block within each category. Write multiple tree groups.

3. **Block-switch commands** (2 days): emit block-switch symbols in
   the command stream at block boundaries.

## Acceptance criteria

- [ ] NBLTYPES ≥ 2 for metablocks > 12 KiB at quality ≥ 7
- [ ] Block-switch commands correctly encoded and decoded
- [ ] Round-trip correctness preserved
- [ ] Ratio improvement ≥ 2% on heterogeneous inputs
- [ ] `brotli -d` accepts output

## Implementation plan

### Data model

```rust
struct BlockPlan {
    /// Block boundaries (positions where a new block starts)
    boundaries: Vec<usize>,
    /// Huffman table assignment per block
    lit_trees: Vec<usize>,
    cmd_trees: Vec<usize>,
    dist_trees: Vec<usize>,
}
```

### Modified: `encode_huffman_chunk_into`

After parsing commands into a list, partition them into blocks. Build
Huffman tables per block. Emit block-switch commands at boundaries.

### Block-switch encoding

Per RFC 7932 §9.3, block-switch commands are emitted BEFORE the block
length expires:
1. Read a block-type code from the block-type Huffman tree
2. Read a block-length code to get the new block's length
3. Continue decoding with the new block's tables

The encoder must track `block_length` for each category and emit
switch commands when it reaches 0.

## Test plan

- Unit test: block boundaries produce valid switch commands
- Unit test: decoder reconstructs correct tables per block
- Integration: heterogeneous input compresses better with blocks
- Integration: `brotli -d` accepts output

## References

- RFC 7932 §9.3 (block types and block-switch commands)
- Upstream: `brotli/c/enc/encode.c:InitOrStitchToBlockBackwardReferences`
- Our decoder: `decoder_full.rs:BlockTypeState` (already handles
  multi-block-type metablocks)
