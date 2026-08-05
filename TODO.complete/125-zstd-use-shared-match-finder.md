# TODO 125: Migrate ZSTD encoder to shared match finder

## Problem

`omnizip-zstd/src/encoder/match_finder.rs` is 1100+ LOC — the
largest match finder in the workspace. It has two variants (fast
and chain-walking), both with prefix-aware (dict) versions, all
duplicated from the shared pattern.

## Proposed fix

Refactor to use `HashChainMatchFinder` for the core, keeping the
ZSTD-specific concerns in `omnizip-zstd`:

- `RawSequence { literal_length, match_length, offset }` — ZSTD
  LZ77 token shape.
- `SeqStore` — accumulator for literals + sequences.
- Repeat-offset tracking (ZSTD has 3 repeat offsets).
- Dictionary-prefix seeding (`seed_prefix` method).
- Strategy-specific parsers (`compress_block_fast`,
  `compress_block_lazy`, `compress_block_lazy2`).

## Acceptance criteria

- [ ] ZSTD encoder uses `HashChainMatchFinder` for hash + chain
  management; codec-specific concerns stay in `omnizip-zstd`.
- [ ] All 174 ZSTD tests pass byte-identical.
- [ ] LOC reduction in `omnizip-zstd` of ≥ 400 lines.

## Priority

P1 — biggest DRY win in the workspace. Also the most complex
migration; do TODOs 122/123/124 first to validate the shared API
shape.

## Dependencies

- TODO 114.
- TODOs 122, 123, 124 (smaller migrations validate the API).
