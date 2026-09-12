# Task 22 — zstd: port the reference's literal table build (HUF_setMaxHeight)

Status: pending (scoped 2026-09-12)
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
