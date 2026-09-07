# Ref-Parity Board — the Inequality Score

Ranked by **I = T × S**: times-slower × size-larger vs the reference
implementation (CPU-time ratio × size ratio; measured 2026-09-07,
9-file corpus × brotli{1,5,9,11} + zstd{1,6,19} + xz-6, single quiet
runs, 10x-loop re-measure for sub-0.05s cells).

- **I = 1.0** — a perfectly even trade (e.g. 2x slower for half the size).
- **I < 1** — net win.
- **S > 1 with T > 1** — paying time AND shipping larger bytes: the
  worst class; no trade exists at all.

Full table: `sweep-inequality-v4-2026-09-08.txt` (v4 — post v0.21.66
getenv hoist + v0.21.67 reference lazy parser; zstd has NO whack
cells left, worst zstd cell I=6.9 under box load. Methodology
identical to v3: ours RUNS=10 in-process, reference 20x CLI loop,
measured under shared-box load ~22-42 so T is inflated vs a quiet
box. Earlier tables kept for the record:
`sweep-inequality-v3-2026-09-07.txt` (amortized both sides; the v1
single-run table `sweep-inequality-2026-09-07.txt` overstated
small-file T ~4x).)

## Tasks

| # | Task | Worst cells | Status |
|---|---|---|---|
| 01 | brotli q4-9 routing: greedy for all (was I=25-85 on sub-1MiB/text) | rfc/dbdump/plists/words/rustsrc q5+q9 | done 2026-09-07 (I -> ~1-2; sizes +6-26%) |
| 02 | brotli q1 on >=1MiB text (22x) | words q1 | pending |
| 03 | zstd fast/intermediate tiers (biggest cluster: 10 cells I=17-29) | all files L6 | done 2026-09-08 (v0.21.67; reference lazy parser, S 0.949-1.009, L6 5-46x faster; dfast/btlazy2 follow-ups listed) |
| 04 | brotli q11 small-file contest overhead (12.5x, S>1) | rfc q11 | pending |
| 05 | csv2m time pathology — root cause 1: getenv in opt hot loop | csv2m zstd L6 I=137 | done 2026-09-07 (v0.21.66; T 202x→~112x est; re-score pending quiet box; residual = task 03) |

## Principles

- Every change keeps the byte-determinism requirement (identical
  output across runs/machines/Rust versions); a tier may change its
  output ONCE, in a release, if the I score improves.
- Size regression is acceptable ONLY with a time win that beats it on
  I; time regression is acceptable ONLY with a size win that beats it
  on I. The contest/measurement shields stay.
- Re-measure on the quiet box, CPU time, best-of-N interleaved (the
  repo box is shared — see TODO.remaining/13 notes).
