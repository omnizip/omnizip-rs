# Task 35 — brotli: coarser distance-histogram sampling at q5 (task 35)

Status: done (2026-09-14, shipped v0.21.91)
Board cells: rustsrc/words/plists/csv2m/dbdump brotli q5 (the hot
post-task-34 column)

## Finding

The reference-port distance split (`split_byte_vector` →
`cluster_histograms`' O(m²) queue) at q5 text costs **53% of encode**
on rustsrc (profile: histogram_combine dominated 9,133 of ~10K
samples) for ±0.5% size — but csv2m genuinely needs it (+3.2% without;
its periodic structure rides distance trees). An on/off default or a
content-class gate is not clean (plists = Structured also loses).

## Change

`omnizip-brotli/src/encoder/emission.rs`: the distance split's
`symbols_per_histogram` parameter uses **4096** at q≤5 (vs the
reference's 544) — 8× fewer sampled histograms = 8× smaller O(m²)
clustering. q6+ keeps the reference's 544 (csv2m q9 measured +0.246%
at 6144 — the finer tier needs the density). `BROTLI_DPH` overrides.

## Verification

- Sizes (11 corpus q5 + 100 KB CSV regression fixture): csv2m
  **−0.117% (improves)**, dbdump −0.120%, rfc −0.465%, rustsrc
  −0.016%; icons +0.110% worst; csv_100k +0.042%; binary cells
  byte-identical (split doesn't fire). q9/q11 byte-identical.
- Regression baseline: PASSES (the 6144 attempt failed csv_100k
  +3.44% — 4096 sits below the cliff).
- Timing (quiet box load ~5, interleaved): rustsrc q5 user
  1.09/1.27/1.17s/10 vs 1.62/1.54/1.54 = **−25..−33%**; fresh T:
  rustsrc **3.73→2.20**, plists **3.18→2.09**, csv2m 3.15→2.63,
  dbdump 2.46→~2.28. words 3.82→3.75 (−2%: the dist split wasn't
  dominant there; its bottleneck is the bank matcher itself).
- Gates: 104+1 tests, fmt, clippy, regression.

## Board impact (v32)

rustsrc brotli q5 I 3.8→**2.2**; plists 3.2→**2.1**; csv2m 2.5→2.1;
dbdump 2.4→2.2; words 3.7→3.6.

## Residual

words brotli q5 (3.6): the bank matcher's per-position cost — the
same safe-Rust floor as task 29/30. The distance-split lever is
exhausted at q5; q6+ keeps the reference density.
