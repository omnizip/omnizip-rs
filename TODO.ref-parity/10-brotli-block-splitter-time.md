# 10 — brotli block splitter: 51% of q11 encode, runs per emission measurement

- **Priority:** P1 (biggest single remaining lever)
- **Score evidence (v11):** sqlite q11 T=4.3 — profiling (4,991
  samples) put split_byte_vector at 51% (population_cost 1,516 +
  histogram_combine 1,274 + compare_and_push_to_queue 1,222 +
  cluster_histograms 1,021), vs the zopfli_hq DP at 31% (whose
  getenv storm, 30%, was hoisted in v0.21.75).
- **Status:** log2 table + split memo DONE 2026-09-09 (v0.21.76):
  sqlite q11 T 4.5 -> 3.8, words 1.9 -> 1.4, rfc -12%, all
  byte-identical. Remaining: the clustering structure itself.

## Root cause shape

`measure_emission_bits` → `emit_metablock_from_commands` → the
BrotliSplitBlock port (`splitter::split_byte_vector`) runs the full
histogram-clustering machinery (block-type clustering over literal,
command, and distance histograms) for EVERY emission measurement —
and the q11 contest measures 3-5 candidates a/b per chunk. The
clustering is also quadratic-ish in histogram pairs
(compare_and_push_to_queue / histogram_combine).

## Landed (v0.21.76)

- **log2() table** (bit-exact, built with .log2() itself; counts
  <= 65536): population_cost alone was 30% of sqlite-q11 samples —
  the libm f64::log2 per symbol also taxed bits_entropy and every
  pair-distance in histogram_combine.
- **One-entry split_byte_vector memo** (full-input key): the a/b
  and tree-cap measurements of the same commands re-ran identical
  splits. Only ~4% — the splits are mostly distinct across
  candidates; the table was the real lever.

## Remaining (deeper work)

- The clustering structure itself: histogram_combine /
  compare_and_push_to_queue evaluate pair distances whose per-symbol
  arithmetic survives the log2 fix — the next shape is incremental
  per-pair cost updates (running entropy sums instead of full
  recompute per merge, as upstream's arena + BitCostDistance reuse
  does) and buffer reuse across the 2-3 splits per emission.
- rfc q11's S=1.020 residual is the header-wire gap (task 04 note),
  not the splitter.

## Acceptance

- sqlite q11 T <= 3; fits/rustsrc q11 improve proportionally;
  output byte-identical (pure port-speed work).

## Disposition: the clustering-structure remainder (2026-09-12)

Worked as pure port-speed (byte-identical output everywhere):

- **Allocation bugs fixed.** Both remap loops allocated a scratch
  `Hist::new(data_size)` PER PROBE (the borrow checker demanded a
  `&mut` distinct from the operands); `compare_and_push_to_queue`
  and `histogram_combine`'s merge paid a full `clone()` (heap alloc)
  per pair / per merge.
- **`population_cost_pair`**: the pair evaluation (queue + both
  remap loops) now computes the combo cost as ONE fused walk over
  the two inputs — nonzero count, first-4 values (sparse closed
  forms), and the main-path accumulation all ride a single
  peekable-zip pass, no materialized sum, no bounds checks. The
  histogram-build loops in `split_byte_vector` iterate slices
  instead of indexing.
- **Measured: fits4m q11 −11% (74.69/74.99 → 66.69/66.56 user
  seconds, two interleaved rounds); small files flat** — sqlite q11
  in-process RUNS=5: ~0.9%; plists q11 spread 1.58-1.78 across
  identical binaries (single runs sit in the shared box's ±6-12%
  noise floor; only the ~70s fits runs resolve). The win concentrates
  where the splitter dominates the q11 contest — the board's worst
  cell (fits q11, T 5.4 → ~4.8).

Why no more is available bit-identically: the fits-q11 profile puts
`population_cost_pair` at 54% of samples, but that cost IS the
reference's own arithmetic — the count-scan early-exits at 5
nonzeros (cheap on dense histograms, where the expensive walk was
already single), and every remaining lever changes values or merge
order: running-entropy incremental updates reorder f64 summation;
pair-count pruning changes queue semantics; SIMD reorders
accumulation. The 0.21.76 log2 table was the last safe constant.
Output stayed byte-identical across the corpus (q5-q11 verified).
Ships as v0.21.81 (pure speed: no size change, no cell can regress).

**Status: done 2026-09-12 (v0.21.81).** Acceptance (sqlite T <= 3)
not reached — that last mile is the reference's own arithmetic at
Rust's per-op cost; every remaining lever (incremental sums, pair
pruning, SIMD) changes values or merge order. Reopen only with a
new structural signal, not profile share.
