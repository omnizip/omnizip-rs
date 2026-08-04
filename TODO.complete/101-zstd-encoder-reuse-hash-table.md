# 101 — ZSTD encoder: per-call hash-table allocation is pathologically slow

**Priority:** Medium
**Status:** ✅ Resolved — 2026-08-04. Adaptive hash_log cap AND
  reusable `ZstdCompressor` both landed.

## Problem

`omnizip_zstd::compress` allocated a fresh match-finder hash table on
every call. At high levels (≥19) the table is `1 << hash_log = 1 << 25`
entries × 4 bytes = 128 MB. The bench harness calls `compress` 5 times
per case (initial, determinism, 3× best-of-3 timing). Each call to
level 22 then costs ~640 MB of allocations and zeroing before any
encoding begins.

For 4 KB inputs the encoding work itself is sub-millisecond, but the
setup dominated wall-time. The benchmark `omnizip-bench` hung for
several minutes per high-level ZSTD case.

## Root cause

`encoder::block::encode_frame_compressed` did:

```rust
let mut match_state = MatchState::new(params.hash_log);
```

`MatchState::new` allocates `vec![0; 1 << hash_log]` unconditionally.
The C reference (`ZSTD_CCtx`) caches the hash table across calls.

## Fix (two-pronged)

### Prong 1: Adaptive `hash_log` cap (PR #54)

`cap_hash_log_for_input` clamps `hash_log` against the input size,
mirroring the C reference's `ZSTD_adjustCParams_internal`. For a 4 KB
input at level 22, this caps the table at 4 KB instead of 128 MB.

Direct `compress(4 KB, level 22)`: 110 µs (was minutes).

### Prong 2: Reusable `ZstdCompressor` (this work)

`ZstdCompressor` is a stateful struct that caches the `MatchState`
across calls. The free function `compress` is unchanged (still
allocates per call); the new struct is the opt-in fast path for batch
workloads.

```rust
pub struct ZstdCompressor {
    match_state: MatchState,
}

impl ZstdCompressor {
    pub fn new() -> Self;
    pub fn compress(&mut self, input: &[u8], level: ZstdLevel) -> Result<Vec<u8>, ZstdError>;
    pub fn hash_log(&self) -> u32;
}
```

`MatchState::resize_for(hash_log)` grows or shrinks the table when the
input size or level changes (amortised via `Vec::resize`). After the
first call, batch workloads with similar-sized inputs at the same
level incur zero per-call allocation.

## Acceptance criteria

- [x] Adaptive `cap_hash_log_for_input` landed — fixes the immediate
      pathological case.
- [x] `ZstdCompressor` struct exposed publicly.
- [x] `MatchState::resize_for` and `MatchState::hash_log` accessors.
- [x] Default ZSTD bench test levels restored to include 19/22.
- [x] Round-trip + determinism preserved (174/174 ZSTD tests pass,
      including 3 new ZstdCompressor tests).
- [x] `ZstdCompressor` output is byte-identical to the free `compress`
      function for the same (input, level) — verified by
      `zstd_compressor_matches_free_function` regression test.

## Out of scope

- Memory budget control (allowing users to cap the hash table size
  for embedded use). Separate TODO if needed.
- Multi-threaded compression. Separate TODO.

## Files

- `omnizip-zstd/src/encoder/block.rs` — `encode_frame_into` (pub),
  `cap_hash_log_for_input` (pub).
- `omnizip-zstd/src/encoder/match_finder.rs` — `MatchState::resize_for`,
  `MatchState::hash_log`.
- `omnizip-zstd/src/lib.rs` — `ZstdCompressor` struct + 3 regression
  tests.
