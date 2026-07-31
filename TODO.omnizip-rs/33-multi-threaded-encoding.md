# 33 — Multi-threaded encoding

- **Priority:** P2 (perf for large files)
- **Depends on:** [11](11-lzma-phase-b-encoder.md), [14](14-zstd-phase-b-encoder.md)
- **Estimated effort:** 2 weeks
- **Location:** per-crate `mt` feature

## Goal

Parallel encoding for large files (> 1 MB). Split input into N chunks,
encode each in a rayon worker, concatenate frame sequences. Mirrors
reference `xz --threads` and `zstd --threads`.

## Approach

ZSTD and XZ both define multi-threaded frame formats:

- **ZSTD**: each thread produces a complete frame; the file is a sequence
  of frames. The decoder processes them in order. This is the simplest
  scheme and is already part of the ZSTD spec.
- **XZ**: each thread produces a block; the file is one stream with N
  blocks. The block index is at the end.

For both: split input into ~1 MB chunks (configurable), encode each in
parallel, concatenate. The decoder doesn't need to know about threading —
the format handles it.

## Phase scope

1. **ZSTD multi-frame** (1 week): split input into N chunks, encode each as
   a single-segment ZSTD frame, concatenate. The decoder already handles
   multi-frame files (Phase A).
2. **XZ multi-block** (1 week): split input into N chunks, encode each as
   an XZ block within one stream, write the multi-block index.

## Acceptance

- `mt` feature compiles with `rayon` as the only new dep.
- On a 4-core machine, encoding a 100 MB file at level 6 is ~3x faster vs
  single-threaded.
- Output is deterministic (same input ⇒ same output regardless of thread
  count). This requires sorting the chunks before concatenation.
- Round-trips through the single-threaded decoder.
- Clippy clean, no `unsafe`, deterministic.

## Implementation notes

- **Determinism is critical.** Rayon's work-stealing doesn't affect output
  because each chunk is independent; the concatenation order is the chunk
  order, not the completion order.
- Chunk size is a tradeoff: smaller chunks → more parallelism but worse
  ratio (less context); larger chunks → better ratio but less parallelism.
  Default 1 MB; configurable.
- Memory: each thread holds a chunk + match finder state. With 8 threads
  at 1 MB chunks + 64 MB match finder, peak is ~512 MB. Document this.
- The decoder does NOT multi-thread in this task. Decode is already fast
  enough single-threaded. If we need parallel decode later, that's a
  separate task.
