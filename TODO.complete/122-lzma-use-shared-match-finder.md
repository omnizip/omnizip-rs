# TODO 122: Migrate LZMA encoder to shared match finder

## Problem

`omnizip-lzma/src/encoder/match_finder.rs` (~415 LOC) duplicates the
hash-chain + word-at-a-time pattern that's now in
`omnizip-codecs::matchfinder::HashChainMatchFinder`.

The LZMA-specific bits (`LzmaMatch { distance, length }` shape, the
explicit `reset()` method, level-aware tuning via `LzmaOptions`) can
stay in `omnizip-lzma` as a thin adapter.

## Proposed fix

1. Replace the body of `omnizip-lzma/src/encoder/match_finder.rs`
   with an adapter that wraps
   `omnizip_codecs::matchfinder::HashChainMatchFinder`.
2. Keep the existing `Match`, `MatchFinder` names (and the methods
   `Lzma1Encoder` calls) so no other LZMA code changes.
3. Verify byte-identical encoder output via the differential tests.

## Acceptance criteria

- [ ] LZMA encoder uses `HashChainMatchFinder` internally.
- [ ] All 142 LZMA tests pass byte-identical.
- [ ] LOC reduction in `omnizip-lzma` of ≥ 200 lines.

## Priority

P2 — pure DRY win, no behaviour change.

## Dependencies

- TODO 114 (shared match-finder module) — landed in 0.14.9.
