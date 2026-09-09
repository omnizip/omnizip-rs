# 12 — fits brotli q11: 87% of encode in the H10 binary-tree walk

- **Priority:** P1 (the heaviest single cell left: fits q11 T=5.1-5.5)
- **Score evidence (v14):** fits4m brotli q11 **I=5.1 (T=5.1x,
  S=1.002)** — 75-81s user for 4 MB. Profiling (one window): 87% of
  samples in `BinaryTreeMatchFinder::store_and_find_capped` under
  `zopfli_hq::collect_matches` — the H10 match collection, not the
  DP nodes and not the emission.
- **Status:** pending (one approach tried and reverted — see below)

## Negative result (2026-09-09, confirmed twice with verified rebuilds)

Window-forming the tree's byte compare (`match_len_from` — two
Option bounds checks per compared byte, the shape that won 5-8x in
zstd's count and the bank's match_len_scan) is NEUTRAL on fits q11:
75.9s vs 75.0-76.8s committed, ±1s repeatability, 25-crate rebuilds
verified both sides. The walk's cost is NOT the compare arithmetic.

Ruled out so far:
- compare form (above — neutral),
- window size: the reference's one-shot H10 also spans the full
  input (num_nodes = min(1<<lgwin, input_size); lgwin 22 at 4 MB),
- algorithm shape: depth cap 64, comp cap 128, 17 bucket bits,
  0x1E35A7BD hash — all identical to H10DefaultParams; the
  reference's own walk compare is ALSO a per-byte iter().zip() loop,
- candidate sharing: collect_matches runs ONCE; both hq passes
  consume the same (num_matches, matches).

Remaining suspects (next probes, in order):
1. **Instrumented node counting**: total tree-node visits and
   compared bytes on a 512 KB FITS slice; if visits saturate the
   depth cap on the 2880-byte header chains, the C pays the same
   count and the gap is per-node constant — else our tree somehow
   holds more candidates (bucket behavior on wrap?).
2. **Per-node stepping constant**: forest[] loads are 32 MB of
   random access; if the C's forest fits cache better (num_nodes =
   1<<lgwin even when input is larger? verify SanitizeParams'
   one-shot lgwin clamp for 4 MB), the walk cost diverges with
   cache-miss rates. Measure ours at fits sized 4 MB vs 512 KB and
   check the scaling exponent.
3. The `out.last()`/push bookkeeping per node (cheap, but the only
   non-C-shaped work left in the loop body).

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
