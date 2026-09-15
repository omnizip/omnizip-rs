# Task 43 — board-integrity fix + the q1/zstd-L1 fidelity floors

Status: done (2026-09-15, docs + board v39; no code change)

## The board bug (mine)

The v34-v38 board generators keyed cell updates on `real/<file>` while
the table rows carry bare filenames — noto/sqlite/plists/install/icons
kept STALE v33 values through four revisions (their "q5 = 2.1-2.35"
rows were pre-task-41 numbers; the code had long since moved). v39
re-scores every real/* brotli cell with this session's measurements:
all q5/q9 rows are fragment-band values (0.1-0.35), the sub-2MiB q11
rows are gated values (0.1). **21 cells >1.3, median I 0.80** — all
remaining are brotli q1 x3 + the zstd L1/L6/L19 columns. xz-6 is
fully at parity.

## The two fidelity audits (both floors confirmed)

1. **brotli q1 (words 1.37, csv2m 1.4, rustsrc 1.46)**: 6s sample of a
   500-iteration words-q1 loop — 92% of samples sit in
   `vendored_compress`'s inlined CreateCommands loop (the
   transliteration itself; the a7565d8-era word-stepped primitives
   already landed). Buffer alloc/zero/free ≈ 3%, memcpy ≈ 3%. A
   buffer pool would buy ≤4% — the cells would still sit ≈1.32.
   Floor.
2. **zstd L1**: `compress_block_fast4_with_prefix` IS the reference's
   shape — single hash table, no chains, step_size 2 (their
   acceleration), rep checks per position (as ZSTD_fast does). The
   tier is faithful; words L1's 3.65x is the per-op cost of the
   faithful algorithm (51% parse + 29% seq emission, both
   scalar-bound, count already word-stepped). Floor.

## Standing

Every remaining >1.3 cell is now a CONFIRMED faithful-tier floor:
brotli q1 (transliteration loop), zstd L1 (fast parser), L6 (lazy2),
L19 (btopt DP), L6-fits (task-21 gate). portable_simd (E0654
re-verified on 1.98 stable this session; canary watching) is the
single mechanical unlock for all 21.
