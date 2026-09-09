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
