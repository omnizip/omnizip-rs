# 05 — csv2m-class periodic data: cross-codec time pathology

- **Priority:** P0 (contains the single worst cell on the v3 board)
- **Score evidence (v3, amortized both sides):** csv2m zstd L6
  **I=137** (T=202x, S=0.681); csv2m zstd L1 I=34 (T=34x); csv2m
  zstd L19 I=11; csv2m brotli q1 I=24.5 (T=29x). The same file is the
  time bomb under EVERY codec — the periodic structure (7,000-row
  period) drives our parse shapes into pathological rescoring.
- **Status:** pending

## Root cause hypothesis

Periodic data produces near-perfect matches at every position; our
zopt-style DP/bank scoring re-evaluates long candidate chains per
position (the 0.16.x-era zeros-quadratic class). The reference's
tiered hashers (dfast/lazy/greedy) have per-position work that is
BOUNDED regardless of match quality — they ride the period once and
skip (the RLE hash-poisoning guard we ported for brotli q5+:
`StoreRange` skips [pos+2, pos+len-4*dist) when dist < len/4).

## Plan

1. Profile csv2m zstd L6 (sample + frame pointers) — confirm whether
   the cost is candidate-chain rescoring, emission, or table rebuild.
2. Port the reference's **RLE/period guard into the zstd match
   finder** (the brotli greedy tier already has it — csv2m brotli q5
   is I~3x, fine; zstd's finder lacks it).
3. Same question for brotli q1 on fits/csv2m (task 02 overlap): the
   two-pass stored-block path may rebuild per block on periodic data.
4. Bounded-work check per CLAUDE.md Invariant 1: the guard must not
   hang on adversarial near-periodic input (test with the #388/#408
   fixture classes).

## Acceptance

- csv2m cells: zstd L1/L6/L19 and brotli q1 all at I <= 10; the
  periodic-structured-text regression fixtures still pass (they pin
  the hang class); byte-determinism green.
