# Task 22 — zstd: port the reference's literal table build (HUF_setMaxHeight)

Status: done (2026-09-13, shipped v0.21.85) — via the cheaper insight
below; the setMaxHeight rebalance itself turned out unnecessary.

## What shipped instead

Package-merge's optimality only matters when the unlimited Huffman
tree EXCEEDS the 11-bit cap — for a fitting tree the plain tree depths
ARE the constrained optimum, so PM's 11-level coin machinery ran for
nothing almost everywhere. `unlimited_huffman_lengths` (two-queue
merge, HUF_buildTree shape, O(m) after a total-order sort — freq desc,
byte asc for determinism) now builds the unlimited tree; if its depth
fits 11 bits those lengths are used directly, and package-merge runs
only on genuinely skewed sections (plus the existing Kraft-sum
fallback). The setMaxHeight port remains documented as the follow-up
if overflow sections ever show up hot — on this corpus they are rare
enough that the measured deltas are tie-ordering noise, not limiter
differences.

## Measured

- Sizes: 33-cell sweep — deltas vs v0.21.84 are +-<=75 bytes (signs
  mixed: fits L19 +47, plists L19 -75, fits L6 -5, most cells
  byte-identical); all round-trips via own decoder + zstd CLI ok.
- Time (quiet box, load ~5, RUNS=100 user CPU): fits L1 3.90/3.35s ->
  2.90/2.87s (**-14..-22%**); fits L19 -6%; fits L6 flat (smaller
  literal share).
- Fresh quiet-box T (both bases >=1s): fits zstd L1 3.5 -> **3.0**,
  L6 3.1 -> **2.6**, L19 1.9 -> **1.4**. v23 board: 72/77 cells <=3.
- Gates: 191+1 tests, fmt, clippy (CI invocation).

## Impact on the reopen chain

This was task 21's dependency #1. It cuts the half-split's cost and
the splitter's per-candidate emission cost, but task 21 still needs
its dependency #2 (incremental split estimates in derive_splits — the
full-encode trials were ~75% of splitter time, now cheaper but still
dominant). Chain order unchanged.

Original scope:
Unblocks: task 21 (L5-L12 seq splitting, −13.8% fits L6 banked), task 19's
residual (package-merge sorts), cheaper task 20 half-split.

## Why

Our literal Huffman lengths come from package-merge — optimal but
expensive: 11 levels × sort of ~510 coins per section, and the cost
multiplies everywhere sections multiply (task 20's half-split doubles it
on divergent blocks; task 21's splitter needs ~130 builds per block).
The reference builds a normal Huffman tree then rebalances with
`HUF_setMaxHeight` — O(m·L), no sorts — which is why ref can afford
24 KiB partitions at 50 ms/4MB.

Reference: `~/src/external/zstd/lib/compress/huf_compress.c`
(`HUF_buildCTable`, `HUF_setMaxHeight`), version 1.6.0-dev — see task
21's version-drift note before trusting gates; the algorithm itself is
stable across 1.5.x/1.6.x.

## Plan

1. Port `HUF_setMaxHeight` as `huffman::set_max_height` (keep
   package-merge as the default; select via env `ZSTD_HUF_SMH=1`).
2. Verify length equivalence class on the corpus: sizes will change
   slightly where the two length-limiters differ — sweep all 33 cells,
   require no cell regresses >0.2% and the L6/L1 fits-class improves
   net after the splitter chain.
3. A/B time (hot-loop user CPU, quiet box): expect build_weights' share
   (~25% of L1 post-.83) to drop by more than half.
4. If S is neutral-or-better and T drops: flip the default, then reopen
   task 21's chain (incremental split estimates → gate extension →
   L5-L12 corpus sweep → v23 board → release drill).

## Non-goals

Matching ref's exact tie-breaking is NOT required (we are not
byte-frozen vs any prior release); determinism within a release is the
only hard constraint.
