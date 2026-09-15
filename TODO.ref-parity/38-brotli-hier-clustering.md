# Task 38 — brotli: hierarchical centroid reduction + f32 cost model in the block splitter

Status: done (2026-09-15, shipped v0.21.93)
Board cells: fits brotli q11 I 3.4→**~0.6** (now FASTER than ref), csv2m
q11 2.6→1.43, words q11 →1.18; fits q9 →~1.04, words q9 →~1.5

## Findings

1. The q10/11 literal-tree assignment's Option R (reference-style
   `cluster_histograms` over the (block,ctx) rows) and the split's own
   `cluster_blocks` both end in a FLAT final combine that is O(k²) pair
   evaluations, each walking two full histograms — bandwidth-bound.
   fits q11: 15,262 survivors of the 64-wide batch loop = **116M
   evaluations ≈ 19-20s** (81% of the post-task-37 encode); csv2m:
   6,938 centroids = 24M ≈ 3s (54%).
2. The f64-exact cost model cost a 512KB log2 table (L2 thrash) + a
   libm call for every bin count > 65,536. The reference computes the
   same costs in **f32** with FastLog2 (256-entry table). Converting to
   f32 alone was timing-NEUTRAL (the loop is memory-bound, not
   arithmetic-bound) — kept anyway: it removes the libm/table
   dependency and makes the values deterministic cross-platform
   (degree-10 Chebyshev log2, max err 1.4e-9, pure IEEE arithmetic).

## Change

`omnizip-brotli/src/encoder/block_splitter.rs`:
- `hier_reduce`: while >2048 centroids survive the batch loop,
  re-window them in 64-wide batches force-merged to ≤16 each, then run
  the flat final combine on the reduced set. Applied before BOTH final
  combines (`cluster_blocks` + `cluster_histograms`). The final remap
  (every row vs every final cluster) is untouched and exact.
  `BROTLI_HIER_CLUST=0` restores the flat combine.
- Cost pipeline (bit_cost / bits_entropy / population_cost /
  population_cost_pair / cluster_cost_diff / find_blocks) converted
  f64→f32 with `fast_log2` (256-entry table + poly_log2).
- Bug fixed en route: `cluster_blocks`' `new_index` was sized by the
  (now reduced) survivor count while block symbols hold ABSOLUTE ids —
  sized by `all_histograms.len()` now.

## Verification

- Corpus q5/q9 × 11: **byte-identical** (small k never enters hier).
- q11: fits +0.042%, sqlite +0.096%, csv2m **−0.868%** (better
  clustering), plists −0.037%, words/rustsrc/rfc/dbdump/install/icons
  ≈0. All round-trip via reference decoder.
- Gates: 104+1 tests, fmt, clippy (CI command).
- Timing (interleaved user CPU, load 74 — brutal but symmetric):
  fits q11 **7.86s vs ref 13.24s → T 0.59** (I ≈ 0.59; was T 1.90
  post-37, 3.4 at v33); csv2m q11 T 3.26→**1.81** (I 1.81×0.789 =
  1.43); words q11 T **1.18** (I 1.18); rustsrc q11 T 1.35 (I ≈1.37,
  within load noise of its 1.3 board value); fits q9 T 1.09.
  R-combine on fits: 19.4s → **0.6s**; csv2m 2.96s → 0.05s.

## Residual (>1.3 after this)

csv2m q11 (1.43): three emissions still run (a + b-split + treecap —
its a-emission is cap-sensitive so task 37's skip doesn't fire; b wins
by 31%, so a margin rule "skip treecap when a_bits > 1.08×win_bits"
would kill the third — measured margin headroom needed). words q9
(~1.5): greedy emission per-op floor. rustsrc q11 (~1.35): same class.
