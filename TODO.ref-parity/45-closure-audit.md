# Task 45 — closure audit: every remaining angle enumerated and disqualified

Status: closed (2026-09-16; terminal)

The board v39 terminus was challenged once more (2026-09-16); the
unexplored angles and their disqualifying measurements:

1. **Nightly-opt-in SIMD** (`#[cfg(feature = "simd-nightly")]`): the
   board measures default builds; an opt-in path moves nothing for
   consumers on stable. Also partial — integer loops only (~1.2-1.4x);
   FP loops can't SIMD without breaking the determinism contract.
2. **The `wide` crate (stable fixed-lane vectors)**: buys exactly the
   [u64;4]-lane shape already measured FLAT in task 40 (count_abs
   wide-stepping never engages on <32-byte matches; two 32B copies
   eat the ILP). Precedent kills it without a re-run.
3. **Multithreading**: the board canon is USER CPU (`/usr/bin/time`
   user, both sides) — MT adds CPU (overhead) and only helps wall
   time. This is why the brotli q10+ MT path never appeared as a board
   win. Structurally disqualified by the metric, not by engineering.
4. **S-headroom trades** (zstd csv2m L19 S 0.925 etc.): the T gaps are
   ~2x; the S headroom is <=8%. No size-lever closes a time gap.
5. **Ref denominators / LTO / allocations / buffer pools**: measured
   flat or <=4% (tasks 43 + 44's addenda).

Closure spot-checks (2026-09-16, load 18-52 — direction only): zstd
words L19 T 1.56, brotli q1 words 2x at sub-resolution — consistent
with board orderings; no drift.

## Terminal state

- Board v39: 21/77 cells >1.3, median I 0.80. All 21 are confirmed
  faithful-tier floors (brotli q1 transliteration; zstd ZSTD_fast /
  lazy2 / btopt loops), each cross-checked four ways.
- TODO.ref-parity: 45 task files, zero pending/open.
- Reopen condition: the polarity-fixed portable_simd canary turning
  RED (task 44) — its annotation names task 30's loop catalogue and
  the gated cells.
