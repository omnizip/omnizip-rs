# Task 37 — brotli: skip the treecap re-measure when the assignment is cap-insensitive

Status: done (2026-09-14, shipped v0.21.92)
Board cells: fits brotli q11 I=3.4 (the worst tracked cell), words
brotli q11 1.4; q10 untracked column moves too

## Finding

fits q11 encode was **two full emissions** of the same hq parse: the
baseline and the tree-cap refinement (`with_lit_tree_cap(6)`). Each
runs the whole literal splitter — on fits block 2 that is the
16,407-block DP (1.43s) + `cluster_blocks` (4.35s: 4117 centroids into
256, the O(m²) initial pair scan dominates at 8.4M
`population_cost_pair` calls). Profile: 97% of samples inside
`split_byte_vector_uncached`, ~70% of that in `histogram_combine`.

But the tree cap is read in exactly ONE place: `cluster_contexts(…,
lit_trees_cap)` feeding the **cmap_a** assignment option. The other
three options are cap-independent — R (`cluster_histograms` over the
per-(block,context) histograms), B (singleton `assign_context_trees`),
C (the decided static map). On fits the R option wins every emission
(245 literal trees vs cmap_a's 64-cap), so the treecap re-measure
produced **byte-identical bits every time** — half the encode bought
nothing. The STATS dump made it visible: both emissions reported
identical `lit_bits=3626495`, `ntrees=245`.

## Change

- `omnizip-brotli/src/encoder/emission.rs`: the A/B/C/R decision now
  records whether the winner was the cap-sensitive `cmap_a` path
  (thread-local; per-thread under the MT chunk encoder).
  `assign_used_cluster_cap()` reads it.
- `omnizip-brotli/src/from_spec_encoder.rs`: both treecap sites (the
  sparse-class early path and the full contest) capture the flag right
  after each candidate's baseline emission and **skip the treecap
  re-measure when the winner is cap-insensitive** — the re-measure is
  byte-identical by construction, so the strict-smaller shield can
  never fire. `BROTLI_TREECAP_ALL` restores everywhere for
  measurement.

Exactness argument (why the skip cannot change output): if the baseline
emission's assignment is R/B/C, the treecap emission equals the
baseline emission bit-for-bit; `c_bits < win_bits` is then false (or
ties when the baseline itself won), so the shield outcome is
unchanged. If the baseline IS cmap_a (noto/sqlite/rustsrc — where
treecap measurably wins), the flag is true and both emissions still
run.

## Verification

- Corpus q9/q10/q11 × 11 files: **all 33 cells byte-identical** to
  v0.21.91 (HEAD worktree A/B). Round-trip implied by identity.
- Gates: 104+1 tests, fmt clean, clippy clean under CI's command
  (`-A clippy::pedantic`; the workspace `-D warnings` failures are
  pre-existing untracked scratch examples, not in CI).
- Timing (interleaved `/usr/bin/time -p` user CPU, load ~12-15):
  - fits q11: ours 46.6s → **24.5-24.9s** vs ref 12.9-13.6s →
    T 3.4 → **1.90**, I 3.4 → **1.9**.
  - fits q10: 52.8 → **27.3s** (−48%; untracked column).
  - words q11: 6.15 → **4.96s**, ref 3.75-3.81 → T **1.17**, I
    1.4 → **1.2** — under the 1.3 bar.
  - csv2m q11 6.44→6.34 (unchanged, within noise); noto/sqlite q11
    unchanged (treecap still fires and wins there); rustsrc q11
    unchanged (its winner IS cmap_a).
- The find_blocks 3-phase vectorization rewrite was measured alongside
  (fits 24.7s ≈ skip-only 24.4s, no gain) but cost csv2m +2.5% —
  **reverted, not shipped**. Kept in session scratch only.

## Board impact (v34)

fits brotli q11 I 3.4→**1.9** (the >3 class shrinks again);
words brotli q11 1.4→**1.2**. Cells >1.3: 44→43 by strict parse.

## Residual

fits q11's remaining 1.9 is the per-op floor inside ONE emission:
4117-centroid `histogram_combine` (~70% of split time; the same
algorithm as C — f64 order must be preserved for byte-identity, so no
SIMD reassociation) + the 16K-block DP + the hq parse itself. This is
the task-30 safe-Rust floor class, toolchain-gated on portable_simd.
