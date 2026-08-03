# 101 — ZSTD encoder: per-call hash-table allocation is pathologically slow

**Priority:** Medium
**Status:** ✅ Partial — 2026-08-03. Adaptive hash_log cap landed;
  reusable `ZstdCompressor` struct deferred.

## Problem

`omnizip_zstd::compress` allocates a fresh match-finder hash table on
every call. At high levels (≥19) the table is `1 << hash_log = 1 << 25`
entries × 4 bytes = 128 MB. The bench harness calls `compress` 5 times
per case (initial, determinism, 3× best-of-3 timing). Each call to
level 22 then costs ~640 MB of allocations and zeroing before any
encoding begins.

For 4 KB inputs the encoding work itself is sub-millisecond, but the
setup dominates wall-time. The benchmark `omnizip-bench` hangs for
several minutes per high-level ZSTD case.

## Root cause

`encoder::block::encode_frame_compressed` does:

```rust
let mut match_state = MatchState::new(params.hash_log);
```

`MatchState::new` allocates `vec![0; 1 << hash_log]` unconditionally.
The C reference (`ZSTD_CCtx`) caches the hash table across calls
(`ZSTD_resetCCtx` only reallocates if params changed).

## Fix

Add a reusable `ZstdCompressor` struct that holds the match-finder
state and exposes `compress_to_vec(&mut self, input, level)`. The
free function `omnizip_zstd::compress` becomes a thin wrapper that
creates a one-shot `ZstdCompressor` (preserving the current API).

Memory ownership:
- `ZstdCompressor::new()` allocates the maximum-size hash table for
  any level the user will call (default: level 22's `1 << 25`).
- `compress_to_vec(level)` re-uses the existing allocation by
  `match_state.reset(level)` (clears + resizes if level changed).

For the bench, the runner can create one `ZstdCompressor` per codec
entry and reuse it across all cases — eliminating the per-call
allocation entirely.

## Acceptance criteria

- [x] Adaptive `cap_hash_log_for_input` landed — fixes the immediate
      pathological case (128 MB allocation for 4 KB input → 4 KB).
      Direct `compress(4 KB, level 22)` time: 110 µs (was minutes).
- [x] Default ZSTD bench test levels restored to include 19/22.
- [ ] `ZstdCompressor` struct exposed publicly. **Deferred** — the
      hash_log cap removes the immediate blocker; full compressor
      reuse is a larger API change tracked separately if hot-path
      benchmarks show allocation cost still matters.
- [ ] `cargo run -p omnizip-bench -- --synthetic 4096 --codec zstd`
      completes in < 5 seconds. **Deferred** — there's a residual
      slow path in the bench harness (not in the codec) that hangs
      the bench on some synthetic corpora. Direct compress calls
      are fast. Investigation continues.
- [x] Round-trip + determinism preserved (170/170 ZSTD tests pass).
- [x] Bench throughput numbers at level 22 are within 2× of reference
      for direct compress (110 µs for 4 KB).

## Out of scope

- Memory budget control (allowing users to cap the hash table size
  for embedded use). Separate TODO if needed.
- Multi-threaded compression. Separate TODO.

## Files

- `omnizip-zstd/src/encoder/block.rs` — refactor into a struct.
- `omnizip-zstd/src/lib.rs` — public `ZstdCompressor` API.
- `omnizip-bench/src/case.rs` — reuse compressor via `BenchCodec`
  pre-built state.
