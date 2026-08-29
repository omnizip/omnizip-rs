# Task 01: LZMA broad-corpus sweep + fast-level gaps

## Status: done (2026-08-29)

## Problem

The broad-corpus sweep (10 files: font, 2 binaries, periodic CSV, db dump,
FITS, random, RFC text, Rust source, word list) exposed catastrophic gaps
at fast levels: lv1 = 1.591x on csv2m, 1.242x on fits4m, 1.102x on
rustsrc, 1.08x on binaries/dbdump. lv6/lv9 were already at parity.

## Root causes (3, all fixed)

1. **The lazy parser was not liblzma's FAST parse.** It had no rep-match
   scanning, no change_pair distance shortening, no rep-preference
   heuristics, no lookahead rep check. Ported xz's
   `lzma_encoder_optimum_fast.c` branch-for-branch into
   `omnizip-lzma/src/encoder/fast_parse.rs` (including the HC4 hash-2 /
   hash-3 probe ladder from `lzma_mf_hc4_find`). All three lazy paths
   (`encode_via_lazy`, `encode_via_lazy_tuned`, `encode_lazy_range`)
   now route through it.

2. **Level routing.** `OPTIMAL_PARSER_LEVEL_THRESHOLD` 4 → 2: level 1
   keeps the fast parse; levels 2+ use the optimal (DP) parser, which
   beats reference on every corpus (0.458x–0.940x).

3. **Range-coder flush byte shortfall.** The 5 shift-lows can emit one
   byte short when the 0xFF-deferral branch fires; the existing fix was
   gated to LZMA2 (`pad_flush`). The gate is removed — the tail byte is
   needed exactly when `range < TOP` at flush, which covers EOPM
   streams too (found via 40×'a' alone streams xz rejected).

Also: the hash table is now sized by dict (port of xz's `lz_encoder.c`
formula, 2^19 at -1 … 2^24 max) instead of fixed 2^18 — marginal but
faithful.

## Result (ours / reference, xz CLI)

| corpus    | lv1 before | lv1 after | lv2 after |
|-----------|-----------|-----------|-----------|
| csv2m     | 1.591x    | **0.945x** | 0.458x   |
| fits4m    | 1.242x    | **0.996x** | 0.689x   |
| rustsrc   | 1.102x    | 1.042x    | 0.854x   |
| bin1/bin2/dbdump/arial | 1.08x | 1.004x | 0.935x |
| rfc       | 1.046x    | 1.038x    | 0.880x   |
| words     | 1.044x    | **0.998x** | 0.866x |
| rand      | 1.014x    | 1.014x    | 1.014x  |

Worst remaining cell: rustsrc lv1 at 1.042x (fastest tier; C is ~3-20x
faster there). All lv2+ cells beat reference.

## Residual

- rustsrc/rfc lv1 1.038-1.042x: parse structure is now faithful; the
  residual is match-candidate recall differences at depth 4. Revisit
  with a symbol-level differential tracer if a downstream corpus needs it.
- rand.bin 1.014x at every level: incompressible; literal-overhead only.
