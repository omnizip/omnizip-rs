# TODO 114: Shared match-finder in omnizip-codecs (DRY)

## Problem

Hash-chain match finders are duplicated across four codec crates:

| Crate | File | Lines | Notes |
|-------|------|------:|-------|
| omnizip-lzma | `src/encoder/match_finder.rs` | 350 | Word-at-a-time, `nice_match`, `max_chain` |
| omnizip-zstd | `src/encoder/match_finder.rs` | 1100 | Two variants: fast + chain-walking, both with prefix-aware dict versions |
| omnizip-lz4 | `src/hc.rs` | 320 | Word-at-a-time, lazy look-ahead |
| omnizip-libdeflate | `src/deflate_lz77.rs` | 530 | Hash-chain, lazy |

That's ~2300 lines of subtly-different implementations of the same
core algorithm. Bug fixes (like the recent backward-extension
infinite-loop in ZSTD) have to be applied four times.

## Proposed fix

Extract a reusable `MatchFinder` in
`omnizip-codecs/src/matchfinder.rs`:

```rust
pub struct HashChainMatchFinder<'a> {
    data: &'a [u8],
    head: Vec<u32>,
    prev: Vec<u32>,
    // ... configuration knobs
}

impl<'a> HashChainMatchFinder<'a> {
    pub fn new(data: &'a [u8], config: MatchFinderConfig) -> Self;
    pub fn find_match(&self, pos: usize) -> Option<Match>;
    pub fn advance(&mut self) -> Option<usize>;
    // ... shared by all four codecs
}

pub struct MatchFinderConfig {
    pub dict_size: u32,
    pub min_match: u32,
    pub max_chain_length: u32,
    pub nice_match: u32,
    pub hash_log: u32,
}
```

Each codec keeps its own LZ77 token format (ZSTD's `RawSequence`,
LZMA's `Match`, etc.) but shares the match-finder internals via an
adapter. The adapter lives in each codec, not in `omnizip-codecs`
(OCP).

## Migration plan

1. Implement `HashChainMatchFinder` in `omnizip-codecs` with the same
   API and behaviour as the LZMA match finder (most general).
2. Refactor `omnizip-lzma` to use it. Verify bit-identical output.
3. Refactor `omnizip-lz4-hc`. Verify.
4. Refactor `omnizip-libdeflate`. Verify.
5. Refactor `omnizip-zstd` (most complex — chain-walking variant
   needs preserving). Verify.
6. Delete the old per-crate implementations.

## Acceptance criteria

- [ ] `omnizip-codecs::matchfinder::HashChainMatchFinder` lands.
- [ ] LZMA, LZ4 HC, libdeflate, ZSTD all use it.
- [ ] All codec tests pass bit-identical.
- [ ] Total LOC in workspace drops by ≥ 1500.
- [ ] Future bug fixes (chain-walking, word-at-a-time, etc.) land in
  one place and benefit all codecs.

## Priority

P1 — pure DRY win, no behaviour change. Big maintainability boost.
