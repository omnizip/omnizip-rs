# TODO 123: Migrate LZ4 HC to shared match finder

## Problem

`omnizip-lz4/src/hc.rs` (~320 LOC) reimplements hash-chain match
finding with the same word-at-a-time extension that's now in
`omnizip-codecs::matchfinder::HashChainMatchFinder`.

## Proposed fix

Refactor `MatchFinder` in `hc.rs` to wrap
`HashChainMatchFinder`. The LZ4-specific bits (`RawMatch` struct,
SENTINEL=u32::MAX, lazy look-ahead) stay in `omnizip-lz4`.

## Acceptance criteria

- [ ] LZ4 HC uses `HashChainMatchFinder`.
- [ ] All 12 LZ4 tests pass byte-identical.
- [ ] LOC reduction in `omnizip-lz4` of ≥ 150 lines.

## Priority

P2.

## Dependencies

- TODO 114.
