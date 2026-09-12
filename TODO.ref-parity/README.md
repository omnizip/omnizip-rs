# Ref-Parity Board — the Inequality Score

Ranked by **I = T × S**: times-slower × size-larger vs the reference
implementation (CPU-time ratio × size ratio; measured 2026-09-07,
9-file corpus × brotli{1,5,9,11} + zstd{1,6,19} + xz-6, single quiet
runs, 10x-loop re-measure for sub-0.05s cells).

- **I = 1.0** — a perfectly even trade (e.g. 2x slower for half the size).
- **I < 1** — net win.
- **S > 1 with T > 1** — paying time AND shipping larger bytes: the
  worst class; no trade exists at all.

Full table: `sweep-inequality-v22-2026-09-12.txt` (v22 — quiet-box T re-scores only: noto brotli q9 T 3.8→1.33, I 3.9→1.4, the carried value was v6-era load-poisoned; sqlite brotli q11 T→3.52, I 3.5, full-contest class like rfc's accepted trade; **median I=1.40, 70/77 <=3, one cell >4**). `sweep-inequality-v21-2026-09-12.txt` (v21 — post v0.21.84 zstd L1 fast-tier half-split: fits L1 S 1.052→0.994 — 3,576,456 vs ref 3,596,632, now SMALLER than the reference — and plists L1 S 1.021→0.987; both were never parse gaps, the literal-Huffman sections were just 2x ref's size; median I=1.40, 69/77 <=3, one cell >4). `sweep-inequality-v20-2026-09-12.txt` (v20 — post v0.21.83 zstd L1 literal-encoding time: byte-identical output, T-only moves from RUNS-loop A/Bs, fits L1 4.2→3.35, plists L1 3.0→1.5, sqlite L1 2.2→1.1; plus a fresh 50x-loop re-measure of rfc brotli q11 T 4.3→4.0 (the carried value predated .79-.82, all of which touched the q11 path); **median I=1.40, 69/77 <=3, one cell >4** — rfc brotli q11 4.1, the dense-small-text full contest, a deliberate time-for-ratio trade, see task 18). `sweep-inequality-v19-2026-09-12.txt` (v19 — post v0.21.82 binary-class b-skip: output byte-identical everywhere, fits brotli q11 T 4.9→3.4 via a ≥5s A/B ratio (66.44→46.5s); **median I=1.40, 67/77 <=3, zero cells >4.5** — worst cells now fits zstd L1 4.4 and rfc brotli q11 4.4). `sweep-inequality-v18-2026-09-12.txt` (v18 — post v0.21.80 + v0.21.81; fresh sizes on every brotli-q11 cell — v17 predated the .80 dict change (plists 114,479→107,235, rustsrc −1,788, install −388, words +386) — and trusted T ratios where the A/B base run ≥5s: fits/csv2m q11 −9.0% (the .81 splitter fusion), plists −5.3% / words −1.9% (v79-vs-ship); zero whack, median I=1.40, 67/77 <=3, one cell >4.5 (fits brotli q11 I=4.9, T 5.4→4.9); every S>1.02 cell maps to a documented tier trade or the task-04 root-2 clustering residual). `sweep-inequality-v17-2026-09-10.txt` (v17 — post v0.21.79 tree-RLE port; fresh sizes every cell, T carried from v16). `sweep-inequality-v16-2026-09-10.txt` (v16 — the full single-window time sweep). `sweep-inequality-v15-2026-09-09.txt` (v15 — its fits zstd L1 "worst" was a noisy-window artifact; v16 shows 4.4). `sweep-inequality-v14-2026-09-09.txt` (v14). `sweep-inequality-v13-2026-09-09.txt` (v13). `sweep-inequality-v12-2026-09-09.txt` (v12). `sweep-inequality-v11-2026-09-08.txt` (v11 — post v0.21.75). `sweep-inequality-v10-2026-09-08.txt` (v10 — post v0.21.74). `sweep-inequality-v9-2026-09-08.txt` (v9 — post v0.21.73). `sweep-inequality-v8-2026-09-08.txt` (v8 — post v0.21.71/.72, zero whack cells). `sweep-inequality-v7-2026-09-08.txt` (v7 — honest board, 4 whack cells). `sweep-inequality-v6-2026-09-08.txt` (v6 — post v0.21.69; wall-time based, several cells load-poisoned). `sweep-inequality-v5-2026-09-08.txt` (v5 — post v0.21.68 q1 flip; 6 whack cells). `sweep-inequality-v4-2026-09-08.txt` (v4 — post v0.21.66
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
| 04 | brotli q11 contest + S audit: gating, conditional-b, and the tree-RLE port (v0.21.79, S 1.020→1.0136) | tree-shape clustering residual (~1.4%) | done 2026-09-10 |
| 06 | brotli q5 sub-1MiB class: bank hasher missing on that path (plists S=1.246) | plists/install/icons/sqlite q5 | done 2026-09-08 (v0.21.69; S 0.994-1.025, I 2.1-5.3) |
| 07 | fits zstd time class: sub-split (v0.21.72) + window primitives (v0.21.73) | fits L6 (I 7.8->4.2) | done 2026-09-08 |
| 08 | fits brotli q5 large: mis-scored cell (wall-vs-user + load-246 batch) — true T~2x | — | closed 2026-09-08 (methodology fixed in v7) |
| 11 | small-file greedy cells: arena package-merge killed the emission allocator storm | plists/noto q5 | done 2026-09-09 (v0.21.77) |
| 10 | brotli block splitter: log2 table + memo (v0.21.76); clustering remainder done 2026-09-12 (v0.21.81) — alloc bugs fixed, fused pair-cost walk, fits4m q11 −11%; remaining cost = reference arithmetic, not reachable bit-identically | q11 column | done 2026-09-12 (v0.21.81) |
| 12 | fits q11: H10 tree walk; three compare forms measured neutral — disposition ACCEPT with analysis (follow-up: instrumented node counts) | fits q11 (I=5.1) | closed 2026-09-09 (analysis) |
| 13 | noto q9: context clustering hot; flat-matrix rewrite byte-identical but neutral — profile attribution alone insufficient, pattern recorded | noto q9 (I=4.4) | closed 2026-09-09 (investigated) |
| 14 | zstd L1 floor: both remaining leads inspected and deprioritized with evidence (Vec+copy ~5%; literals 19% share sub-threshold) | zstd L1 cells (I 3.0-4.4) | closed 2026-09-10 (inspected) |
| 15 | plists q11 S=1.093: sparse path had dict DISABLED in the base hq parse (ref: 2,189 dict matches, ours: 0) — FIXED; follow-up (2026-09-11): remaining 418-dict-recall traced past gate/finder to DP pricing; upstream-exact symbol pricing A/B'd +1.23% corpus (csv2m q11 +41%) and reverted — ACCEPT residual, leads documented | plists q11 (S 1.024, net-win cell) | done 2026-09-11 (v0.21.80 + disposition) |
| 16 | brotli q10/11 reduced contest: the b (split literal assignment) emission never wins on Binary-class inputs (BTOPT_DUMP winner table: every b win is Text/Structured) — gated on is_text_like, byte-identical, fits q11 −30%; f32-vs-f64 cost-model root-cause hypothesis also measured out (C 1.2.0 FastLog2 is exact f64 — task-15 addendum) | fits brotli q11 (I 4.9→3.4; zero cells >4.5 remain) | done 2026-09-12 (v0.21.82) |
| 17 | zstd L1 literal-encoding time: profile = 43% in encode_literals_internal — O(n²) min-heap computed then discarded + per-coin-Vec package-merge (realloc storm) + per-literal flush. Arena package-merge, lazy fallback, batched flushes; byte-identical 33/33 cells; L1 −14…−50% (fits −20%, plists/sqlite −50%) | zstd L1 T-cluster (fits 4.4→3.5, plists 3.1→1.5, sqlite 2.2→1.1) | done 2026-09-12 (v0.21.83) |
| 20 | zstd L1 S gap decomposed decoder-side: lit_regen totals identical to ref — the whole 186 KB was literal-Huffman section size (ref emits 64 KiB blocks on heterogeneous content at L1). One-halving split on the existing divergence screen (the never-tried middle option between no-split and the v7 16 KiB disaster); fits S 1.052→0.994, plists 1.021→0.987, 9/11 files byte-identical, T ≤+5% | fits/plists zstd L1 (both now size-wins vs ref) | done 2026-09-12 (v0.21.84) |
| 21 | zstd L5-L12 post-parse seq splitting: fully diagnosed (fits L6's whole S gap = 16.32 vs 12.40 bits/seq — one FSE table set per 127 KiB block vs ref's 24 KiB partitions; ref 1.5.7 runs its splitter from Greedy up) and measured (fits L6 −13.8%, S→0.901) but BLOCKED: derive_splits trial-encodes per candidate (~75% of encode) and even a perfect derive leaves ~130 package-merges per block. Reopen chain: cheap literal table build (setMaxHeight port) → incremental split estimates → re-extend gate | fits zstd L6 (S 1.044, T-bound trade for now) | blocked 2026-09-12 (task 21) |
| 22 | port HUF_setMaxHeight (ref's O(m·L) literal-table build) — unblocks task 21's banked −13.8% fits L6, task 19's PM-sort residual, and halves task 20's cost; env-selectable first, default flip only if S neutral-or-better and T drops | zstd L6 column + L5-L12 (pending) | pending (scoped) |
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
