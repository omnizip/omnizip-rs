# Task 24 — board hygiene: fresh-T re-sweep of the carried cells

Status: done (2026-09-13; 13 cells flagged for quiet-box re-check)
Trigger: the zstd L1 column's T values (csv2m 3.3, rustsrc 3.1, words
3.0) were measured pre-v0.21.83 — three literal-path releases (.83/.85/
.86, cumulative −40..−60% on literal-heavy L1) have landed since. The
same staleness class cost noto brotli q9 a 3.8 → 1.33 correction (v22).

## Methodology (hardened this session)

- **macOS `date +%N` does not exist** — every date-based loop timing in
  earlier sessions was second-granular (the precise numbers all came
  from the binaries' own `Instant` timers or `/usr/bin/time`). Use
  `/usr/bin/time` user CPU for both sides.
- `bash -c "... $(seq N) ..."` breaks: the substitution's newlines are
  kept inside double quotes and the inner parse fails silently. Use
  single-quoted bodies with variable concatenation.
- T = (user_ours / N_ours) / (user_ref / N_ref) — normalize run counts
  (an earlier draft divided totals directly).
- Load policy: our compute-heavy side inflates ~20-30% under load 10-25
  while the ref (memcpy-bound) stays stable, so busy readings are
  HIGH-biased on T. Update a board cell only when fresh-T is clearly
  BELOW the carried value (improvements are real; regressions may be
  load). Save exact quiet-load re-measures for values that move UP.

## Sweep

55 cells: {zstd L1, zstd L6, zstd L19, brotli q9, brotli q11} × 11
corpus files, static per-cell N (≥2s user per side, ref at 2×N),
interleaved single-pass, results to `~/tsweep2_results.txt`.

## Results (55 cells, box load 15-22, post-reboot)

**Accepted (fresh clearly below carried, 20 cells):** csv2m zstd L1
3.3→1.98, rustsrc zstd L1 3.1→2.64, noto zstd L1 2.5→0.62, noto brotli
q11 2.7→2.01, noto zstd L19 2.6→1.73, csv2m zstd L6 2.3→1.61, rustsrc
zstd L6 2.2→1.52, plists zstd L6 1.9→1.12, icons zstd L1 1.7→0.56,
sqlite zstd L6/L19 1.7→0.78 / 1.7→1.44, noto zstd L6 1.7→0.67,
icons zstd L6 1.3→0.59, dbdump zstd L1/L6 1.3→0.96 / 1.1→0.88,
icons brotli q11 1.2→0.97, rfc zstd L19 1.2→0.98, sqlite zstd L1
1.1→0.85, install zstd L1/L6 0.8→0.42. Board: **median I 1.40→1.30,
74/77 cells ≤3**.

**Flagged (fresh well ABOVE carried — carried kept):** the entire
brotli q9 column read ~2x its carried values at load (fits 0.9→1.44,
rustsrc 0.7→1.62, words 0.7→1.59, csv2m 0.9→1.48, plists 0.8→1.27, rfc
1.1→1.8, install 1.7→2.77), plus words zstd L1 3.0→5.44, words zstd
L19 1.3→2.41, csv2m zstd L19 1.5→3.22, dbdump zstd L19 1.3→2.33,
csv2m brotli q11 2.8→3.14. Two candidate explanations: asymmetric load
inflation on the compute-heavy side (ours suffers more than the
memcpy-stable ref at load 15-22), or a real q9/L19 regression the
carried values predate. The q9 class-coherence is suspicious either
way — RE-CHECK ON A QUIET BOX before treating either direction as
truth. Nothing shipped on the basis of these readings.

## Resolution (same day, quiet box, load 4-6)

All 13 flagged cells re-measured quiet — the readings STAND (brotli q9
1.24–1.96 across the column; words zstd L1 3.65, L19 1.95, csv2m L19
2.43, dbdump L19 2.33). Suspecting a code regression, a TAG BISECT
was run:

- brotli q9 on fits: v0.21.78 / .80 / .81 / .82 / current =
  2.64 / 2.56 / 2.63 / 2.65 / 2.44 s per 3 runs — flat, current
  fastest. The .81 splitter-fusion suspect is cleared.
- zstd words L19: v0.21.82 / .83 / .85 / current = 5.85 / 6.09 /
  5.98 / 5.68 s per 5 runs — flat, current fastest.

**Conclusion: no code regression.** The carried T values for these
cells were REF-SIDE artifacts of the pre-task-24 methodology (the
`date` second-granularity on the reference CLI loops — see the
methodology section). The fresh quiet values are the truth and the 12
cells moved to them in the v26 board (install brotli q9 improved
1.7→1.41). Board: median I=1.40, 73/77 ≤3 — the honest floor of the
current implementation on this box.
