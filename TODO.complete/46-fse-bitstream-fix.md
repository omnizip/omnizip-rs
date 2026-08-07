# Task 46: FSE bitstream fix (critical path)

## Status: deferred — ZSTD encoder works without this. FSE encoder improvement for better ratio.
## Blocks: 47 (Huffman literals wiring)
## Priority: P0

## Problem

`compress_using_ctable` in `omnizip-zstd/src/fse/encoder.rs` deviates
from C `FSE_compress_usingCTable_generic`
(`~/src/external/zstd/lib/compress/fse_compress.c:551-608`) in 4 ways:

1. Missing mod-4 alignment block (C:579-585)
2. Missing 4-symbol fast path in main loop (C:597-600)
3. Spurious `ip==0` break between s2/s1 encode
4. Conditional flush (C always flushes at loop end)

## Plan

- Rewrite `compress_using_ctable` to match C exactly:
  init → mod-4 alignment → 4-symbol main loop → flush s2, flush s1 → close.
- Remove `#[ignore]` from `round_trip_simple_stream`.
- Delete `verify_block_round_trips` from `encoder/block.rs`.

## Acceptance

- `cargo test -p omnizip-zstd` — 0 failures, 0 ignored.
- ZSTD 500K multi-block input round-trips without Raw fallback.
