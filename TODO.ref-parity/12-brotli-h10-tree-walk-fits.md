# 12 — fits brotli q11: 87% of encode in the H10 binary-tree walk

- **Priority:** P1 (the heaviest single cell left: fits q11 T=5.1-5.5)
- **Score evidence (v14):** fits4m brotli q11 **I=5.1 (T=5.1x,
  S=1.002)** — 75-81s user for 4 MB. Profiling (one window): 87% of
  samples in `BinaryTreeMatchFinder::store_and_find_capped` under
  `zopfli_hq::collect_matches` — the H10 match collection, not the
  DP nodes and not the emission.
- **Status:** pending (one approach tried and reverted — see below)

## Negative result (2026-09-09)

Window-forming the tree's byte compare (`match_len_from` — two
Option bounds checks per compared byte, the exact shape fixed with
5-8x wins in zstd's count and the bank's match_len_scan) measured
NEUTRAL-TO-NEGATIVE on fits q11 (same-load A/B: 76.5s vs 75.1s).
**Lesson: the window form pays ~10-15 ops of setup per call; it wins
on long compares (text matches) and loses on short ones.** The tree
walk on binary is short-compare-dominated (candidates diverge within
a few bytes). Reverted; do not re-apply blindly — the instinct
"same disease, same cure" fails when the compare-length distribution
changes.

## Remaining hypotheses for the 5x

1. **Walk depth / candidate volume**: TREE_DEPTH vs the reference's
   H10 settings; are we visiting more tree nodes per position than
   the C on binary? (Count nodes/position ours vs ref via trace on a
   slice.)
2. **Per-step constant**: the C's H10 walks indices with no bounds
   checks and unrolled 4-byte hash rejects (kHashMul32 gate before
   the compare). Our walk does `match_len_from` per node WITHOUT a
   cheap reject-byte pre-check (the C compares data[pos+len] vs
   data[prev+len] single bytes to prune). Check whether the C's
   store prune differs.
3. **The two-pass q11 loop**: collect_matches runs TWICE (q11
   StartPosQueue passes) — confirm the second pass re-walks the tree
   (it should reuse candidates).

## Acceptance

- fits q11 T <= 3x with byte-identical output; csv2m q11 (3.0)
  improves proportionally.
