# Task 20 — zstd L1: fast-tier half-split (the reference's 64 KiB block point)

Status: done (2026-09-12, shipped v0.21.84)
Board cells: fits zstd L1 (S=1.052), plists zstd L1 (S=1.021)

## Root cause of the S gap (decoder-side decomposition, ZSTD_SEC_DUMP)

The gap was never parsed apart before this task — it was folded into a
"tier floor" statement (task 14's leads were all T-side). Dumping both
streams' per-block structure through our own decoder:

- **lit_regen totals are identical** (ref 4,165,330 vs ours 4,169,774 —
  +0.1%): the parses find the same matches. NOT a match-quality gap.
- **The entire 186 KB gap is literal Huffman efficiency**: ref encodes
  fits L1 literals at ~6.9 bits/lit, ours at 7.25 — because ref emits
  **64 KiB blocks** on this content (62 blocks: first 128 KiB then 64 KiB
  throughout; verified not parametric — `-T1` identical, words.txt gets
  full 128 KiB blocks — it is content-triggered) while our BLOCK_MAX_SIZE
  blocks never split at the fast tier.
- Our sub-split mechanism (16 KiB on divergent halves) exists but is
  gated to opt tiers, because at L1 it was a v7-board time disaster
  (71% encode cost). The one-halving form — ref's actual L1 operating
  point — had never been tried.

## Change

`omnizip-zstd/src/encoder/block.rs`: when a chunk's halves diverge
(the existing TV-distance ≥ 0.25 screen) and strategy == Fast (L1/L2),
emit the chunk as two half-size blocks (one 63.5 KiB halving — no deeper
recursion). Default on (matches reference behavior);
`ZSTD_NO_HALFSPLIT` restores the unsplit emission for measurement.

## Verification

- Sizes: fits L1 3,782,300 → **3,576,456 (−5.44%, smaller than ref's
  3,596,632 — S 1.052 → 0.994)**; plists L1 159,708 → 154,390 (−3.33%,
  S 1.021 → 0.987). The other 9 corpus files byte-identical at L1
  (divergence screen does not fire); L2/L6/L19 byte-identical (gate is
  strategy-scoped).
- Round-trips: our decoder + `zstd -d` CLI byte-exact on changed cells.
- Time: user-CPU RUNS=100 A/B ≤5% (base 2.37/2.50s vs half 2.48/2.50s)
  — PM table builds double on divergent blocks but count_frequencies
  totals are unchanged. Board T taken at ×1.03.
- Gates: 191+1 zstd tests, fmt, clippy (CI invocation), regression
  baseline unaffected (no zstd L1/L2 entries), full CI green.

## Board impact (v21)

fits zstd L1 I 3.52 → 3.43 (S 0.994); plists L1 I 1.53 → 1.51
(S 0.987). Both cells now NET WINS vs the reference on size. Median and
cell counts unchanged (1.40, 69/77 ≤ 3, one cell > 4: rfc brotli q11).

## Notes

- Fresh-T audit for noto brotli q9 and sqlite brotli q11 (carried 3.8 /
  3.3 from v16, predating .79–.83) was ATTEMPTED but the box sat at load
  18–64 (same-binary swings 3x) — inconclusive; re-measure on a quiet
  box. One load-lull sample suggested noto q9 T≈2.0, which would take
  that cell to I≈2.1.
- Deeper parity option (not needed): the reference's literal table build
  is HUF_setMaxHeight (O(m·L) rebalancing), not package-merge — a port
  would cut PM cost further and could make the half-split free, at the
  cost of slightly different (ref-exact) code lengths.
