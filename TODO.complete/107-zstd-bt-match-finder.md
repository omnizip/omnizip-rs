# 107 — ZSTD binary tree match finder for Btopt/Btultra2 (levels 16-22)

**Priority:** P0 — HIGH
**Source:** Performance audit (2026-08-04)
**Status:** ⏳ Pending

## Problem

`encoder/cparams.rs` defines Btopt/Btultra2 strategies for levels 16-22,
but `encoder/block.rs` dispatches all of them to the same `compress_block_lazy2`
function. The reference zstd uses a **binary tree (BT4)** match finder for
these levels, which finds longer matches via O(log n) tree traversal vs
O(chain_length) hash chain walks.

Current dispatch (block.rs:416):
```rust
Strategy::Lazy2 | Strategy::Btlazy2 | Strategy::Btopt | Strategy::Btultra | Strategy::Btultra2 => {
    compress_block_lazy2(chunk, &mut seq_store, ms, params.search_log, params.target_length);
}
```

**Impact:** 5-15% ratio gap vs reference zstd at levels 16-22.

## What helps today

The `cparams` table already specifies different `hash_log`, `chain_log`,
`search_log`, and `target_length` for each level. Even with lazy2, higher
levels search deeper hash chains. The gap is specifically from match
QUALITY, not match SEARCH DEPTH.

## Proposed fix

Implement a BT4 match finder:
- Each position has left/right child pointers in a binary search tree
- Tree is ordered by suffix (most recent at root)
- O(log n) search per position
- Better match quality than hash chains for inputs with many similar contexts

### Algorithm (BT4 from zstd reference)

```
for each position ip:
    hash = hash4(ip)
    match_pos = head[hash]
    walk binary tree from match_pos:
        compare input[ip..] vs input[match_pos..]
        extend to longest match
        if match < best: go left (shorter suffixes)
        if match >= best: go right (longer suffixes)
    update tree: insert ip as child
```

### Phased delivery

1. **Phase 1**: BT4 match finder data structure + insert/lookup
2. **Phase 2**: Wire into block.rs dispatch for Btopt+
3. **Phase 3**: Tuning (prune thresholds, lazy parameters for BT)

## Acceptance criteria

- [ ] BT4 match finder module exists
- [ ] Levels 16-22 produce different output than levels 9-12
- [ ] Level 22 ratio within 10% of reference zstd on Enwik8
- [ ] Round-trip preserved

## Effort estimate

3-5 days.

## Files

- `omnizip-zstd/src/encoder/bt_match_finder.rs` (new)
- `omnizip-zstd/src/encoder/block.rs` (dispatch update)
