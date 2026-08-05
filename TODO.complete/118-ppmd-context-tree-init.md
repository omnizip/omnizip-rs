# TODO 118: PPMd per-call context-tree init profiling

## Problem

`omnizip-ppmd` shows high per-call overhead on small chunks. The
hypothesis (LimniFS proposal #13) is that the context-tree init
cost dominates for inputs < 4 KiB.

Currently the only public API is `compress/decompress` on
`Ppmd7Codec`/`Ppmd8Codec`, which constructs a fresh context tree on
every call.

## Proposed fix

1. **Profile**: add a `cargo bench` for PPMd on inputs of 256 B,
   1 KiB, 4 KiB, 16 KiB, 64 KiB. Identify where the per-call overhead
   becomes negligible.
2. **Reusable state**: introduce `PpmdCompressor` (mirror
   `omnizip-zstd::ZstdCompressor`) that holds the context tree across
   calls. Each call resets adaptation but reuses the allocation.
3. **Cheap reset**: if the context tree is reusable, a "soft reset"
   (clear counters, keep structure) is much cheaper than full
   re-construction.

## Acceptance criteria

- [ ] Bench harness shows per-call overhead < 100 µs after fix.
- [ ] Throughput on 1 KiB inputs improves by ≥ 3×.
- [ ] Differential parity unchanged (same byte output as old API on
  each individual call).
- [ ] `PpmdCompressor` lands with the same API shape as
  `ZstdCompressor`.

## Priority

P2 — only affects small-chunk workloads. LimniFS batches typically
> 64 KiB so this is low priority for the main consumer.
