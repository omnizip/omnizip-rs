# 11 — brotli small-file greedy cells: plists q5 I=5.2, noto q9 I=5.2

- **Priority:** P1 (top uncovered cells on the v12 board)
- **Score evidence (v12):** plists brotli q5 **I=5.2 (T=5.1x, S=1.019)**
  — 0.059s user for 145 KB (2.5 MB/s effective); noto brotli q9
  **I=5.2 (T=5.1x, S=1.031)** — 0.038s for 85 KB. dbdump q5 3.9,
  rfc q5 ~4-5, sqlite q5 ~3.9 same class.
- **Status:** done 2026-09-09 (v0.21.77): the emission was the cost,
  not the parse — HuffmanLengths::build's package_merge allocated a
  Vec<usize> PER COIN, cloned them through the merge loop, and
  rebuilt the original-coin list every level. Arena rewrite
  (byte-identical lengths): plists q5 T 5.1 -> 3.0, sqlite q5 -> 1.3,
  rfc q5 -> 2.0, noto q5 -> 1.7, plists q9 -> 0.9, noto q9 -> 4.3
  (acceptance was <= 3 for both; noto q9's remainder is the q9
  zopfli tier's own emission).

## Root cause (profile-confirmed)

plists q5 (8,236 samples): emission 60% (HuffmanLengths collection
+ build with realloc churn ~12%, assign_context_trees 8%), parse
34% (bank scans). The package_merge allocator storm was the lever.

## Plan

1. Sample plists q5 and noto q9 (RUNS=60 in-process, mid-run sample).
2. Whack what shows; per-frame init costs get hoisted/reused only
   if they dominate (MatchState/structures are per-call today).
3. Output must stay byte-identical (pure speed work).

## Acceptance

- plists q5 and noto q9 T <= 3x; no other cell regresses.
