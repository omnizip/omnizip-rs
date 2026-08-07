# 182: BT4 Binary-Tree Match Finder

## Priority: P1 (ratio improvement at high levels)

## Status: pending

## Context

The LZMA encoder currently uses a hash-chain match finder (via the
shared `omnizip_codecs::HashChainMatchFinder`). At levels 7-9 the C
reference (`xz-utils` `lz_encoder_mf.c`) switches to a binary-tree
(BT4) match finder that finds longer matches via O(log n) sorted
position walks.

## Algorithm (from `lz_encoder_mf.c`)

BT4 maintains a binary tree of positions keyed by their 4-byte hash:

1. Hash 4 bytes at current position → `hash`.
2. Look up `son[hash]` for the most recent position with that hash.
3. Walk the binary tree: at each node, compare the match length against
   the current best. Branch left if the candidate is lexicographically
   smaller, right if larger.
4. Stop after `depth` steps or when a match of length ≥ `nice_len` is
   found.
5. Insert the current position into the tree.

## Files

- New: `omnizip-lzma/src/encoder/bt4_match_finder.rs` (~800-1000 LOC)
- Modify: `omnizip-lzma/src/encoder/lzma1.rs` — dispatch to BT4 at
  levels ≥ 7
- Modify: `omnizip-lzma/src/codec.rs` — map levels 7-9 to BT4

## Acceptance criteria

- [ ] BT4 finds matches ≥ hash-chain quality on all test fixtures
- [ ] Level 9 ratio within 5% of `xz -9` on enwik8
- [ ] Deterministic: same input → same matches
- [ ] All existing tests still pass
- [ ] Round-trip via own decoder + `xz -d`

## Reference

- `~/src/external/xz-utils/src/liblzma/lz/lz_encoder_mf.c` (BT4 section)
- XZ Utils `lzma_mf_bt4_find` / `lzma_mf_bt4_skip`
