# TODO 124: Migrate libdeflate LZ77 to shared match finder

## Problem

`omnizip-libdeflate/src/deflate_lz77.rs` (~530 LOC) has its own
`MatchFinder` separate from the shared one. Plus the recent dynamic-
Huffman work added `collect_tokens` which could just take a
`HashChainMatchFinder` reference.

## Proposed fix

1. Replace `MatchFinder` with `HashChainMatchFinder`.
2. `collect_tokens` accepts the shared finder.
3. Wire both `deflate_fixed_huffman` and `deflate_dynamic_huffman`
   through the shared API.

## Acceptance criteria

- [ ] libdeflate LZ77 uses `HashChainMatchFinder`.
- [ ] All 24 libdeflate tests pass byte-identical.
- [ ] LOC reduction in `omnizip-libdeflate` of ≥ 200 lines.

## Priority

P2.

## Dependencies

- TODO 114.
