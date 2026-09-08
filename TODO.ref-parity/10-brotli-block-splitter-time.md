# 10 — brotli block splitter: 51% of q11 encode, runs per emission measurement

- **Priority:** P1 (biggest single remaining lever)
- **Score evidence (v11):** sqlite q11 T=4.3 — profiling (4,991
  samples) put split_byte_vector at 51% (population_cost 1,516 +
  histogram_combine 1,274 + compare_and_push_to_queue 1,222 +
  cluster_histograms 1,021), vs the zopfli_hq DP at 31% (whose
  getenv storm, 30%, was hoisted in v0.21.75).
- **Status:** pending

## Root cause shape

`measure_emission_bits` → `emit_metablock_from_commands` → the
BrotliSplitBlock port (`splitter::split_byte_vector`) runs the full
histogram-clustering machinery (block-type clustering over literal,
command, and distance histograms) for EVERY emission measurement —
and the q11 contest measures 3-5 candidates a/b per chunk. The
clustering is also quadratic-ish in histogram pairs
(compare_and_push_to_queue / histogram_combine).

## Plan

1. Profile inside the splitter: population_cost vs combine vs queue
   — the C's BrotliBlockSplitterComputeCostsFromArray equivalent and
   where the port differs in allocation shape (per-call Vec churn
   vs the C's arena).
2. Candidate cheap wins:
   a. Reuse buffers across the a/b and candidate measurements (the
      splitter allocates histograms/indices per call).
   b. The literal-assignment override (a/b) only changes LITERAL
      histograms — command/distance splits are IDENTICAL between a
      and b for the same commands: compute them once.
   c. Same for candidates sharing the same parse (hq vs hq+dict
      differ only in some commands... no — different parses; skip).
3. Deeper: port the C's incremental cost updates
   (BrotliBlockSplitterComputeCostsFromArray's running sums) if the
   port recomputes per merge.

## Acceptance

- sqlite q11 T <= 3; fits/rustsrc q11 improve proportionally;
  output byte-identical (pure port-speed work).
