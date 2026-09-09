# Ref-Parity Board — the Inequality Score

Ranked by **I = T × S**: times-slower × size-larger vs the reference
implementation (CPU-time ratio × size ratio; measured 2026-09-07,
9-file corpus × brotli{1,5,9,11} + zstd{1,6,19} + xz-6, single quiet
runs, 10x-loop re-measure for sub-0.05s cells).

- **I = 1.0** — a perfectly even trade (e.g. 2x slower for half the size).
- **I < 1** — net win.
- **S > 1 with T > 1** — paying time AND shipping larger bytes: the
  worst class; no trade exists at all.

Full table: `sweep-inequality-v12-2026-09-09.txt` (v12 — post v0.21.76; **zero whack cells, worst cell I=5.5**; rfc q11 by design, plists q5/noto q9 small-file greedy, fits zstd L1). `sweep-inequality-v11-2026-09-08.txt` (v11 — post v0.21.75). `sweep-inequality-v10-2026-09-08.txt` (v10 — post v0.21.74). `sweep-inequality-v9-2026-09-08.txt` (v9 — post v0.21.73). `sweep-inequality-v8-2026-09-08.txt` (v8 — post v0.21.71/.72, zero whack cells). `sweep-inequality-v7-2026-09-08.txt` (v7 — honest board, 4 whack cells). `sweep-inequality-v6-2026-09-08.txt` (v6 — post v0.21.69; wall-time based, several cells load-poisoned). `sweep-inequality-v5-2026-09-08.txt` (v5 — post v0.21.68 q1 flip; 6 whack cells). `sweep-inequality-v4-2026-09-08.txt` (v4 — post v0.21.66
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
| 02 | brotli q1: two-pass default (was from-spec, I=44.5 on fits) | fits/words/csv2m q1 | done 2026-09-08 (v0.21.68; every q1 cell I=0.3-1.7) |
| 03 | zstd fast/intermediate tiers (biggest cluster: 10 cells I=17-29) | all files L6 | done 2026-09-08 (v0.21.67; reference lazy parser, S 0.949-1.009, L6 5-46x faster; dfast/btlazy2 follow-ups listed) |
| 04 | brotli q11 contest: large-file (v0.21.70) + content-class (v0.21.71) gating done | rfc q11 (I=8.0) + hq DP constant | partial |
| 06 | brotli q5 sub-1MiB class: bank hasher missing on that path (plists S=1.246) | plists/install/icons/sqlite q5 | done 2026-09-08 (v0.21.69; S 0.994-1.025, I 2.1-5.3) |
| 07 | fits zstd time class: sub-split (v0.21.72) + window primitives (v0.21.73) | fits L6 (I 7.8->4.2) | done 2026-09-08 |
| 08 | fits brotli q5 large: mis-scored cell (wall-vs-user + load-246 batch) — true T~2x | — | closed 2026-09-08 (methodology fixed in v7) |
| 10 | brotli block splitter: log2 table + memo DONE (v0.21.76); clustering structure remains | q11 column | partial |
| 09 | zstd L1 emission: bitstream-size counting replaces byte-materializing measurement (v0.21.74, words L1 -41%) | words/rustsrc L1 (I 4.0-4.3) | done 2026-09-08 |
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
