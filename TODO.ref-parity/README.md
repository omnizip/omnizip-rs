# Ref-Parity Board — the Inequality Score

Ranked by **I = T × S**: times-slower × size-larger vs the reference
implementation (CPU-time ratio × size ratio; measured 2026-09-07,
9-file corpus × brotli{1,5,9,11} + zstd{1,6,19} + xz-6, single quiet
runs, 10x-loop re-measure for sub-0.05s cells).

- **I = 1.0** — a perfectly even trade (e.g. 2x slower for half the size).
- **I < 1** — net win.
- **S > 1 with T > 1** — paying time AND shipping larger bytes: the
  worst class; no trade exists at all.

Full table: `sweep-inequality-v3-2026-09-07.txt` (v3 methodology —
amortized timing BOTH sides: ours RUNS=10 in-process, reference 20x
CLI loop; the v1 single-run table overstated small-file T ~4x via our
lazy-init tax and is kept as sweep-inequality-2026-09-07.txt for the
record).

## Tasks

| # | Task | Worst cells | Status |
|---|---|---|---|
| 01 | brotli q4-9 routing: greedy for all (was I=25-85 on sub-1MiB/text) | rfc/dbdump/plists/words/rustsrc q5+q9 | done 2026-09-07 (I -> ~1-2; sizes +6-26%) |
| 02 | brotli q1 on >=1MiB text (22x) | words q1 | pending |
| 03 | zstd fast/intermediate tiers (biggest cluster: 10 cells I=17-29) | all files L6 | pending |
| 04 | brotli q11 small-file contest overhead (12.5x, S>1) | rfc q11 | pending |
| 05 | csv2m-class periodic data time pathology (csv2m zstd L6 I=137!) | csv2m x{zstd L1/L6/L19, brotli q1} | pending |

## Principles

- Every change keeps the byte-determinism requirement (identical
  output across runs/machines/Rust versions); a tier may change its
  output ONCE, in a release, if the I score improves.
- Size regression is acceptable ONLY with a time win that beats it on
  I; time regression is acceptable ONLY with a size win that beats it
  on I. The contest/measurement shields stay.
- Re-measure on the quiet box, CPU time, best-of-N interleaved (the
  repo box is shared — see TODO.remaining/13 notes).
