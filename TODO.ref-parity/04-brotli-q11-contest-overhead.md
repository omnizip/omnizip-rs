# 04 — brotli q11 small-file contest: 12.5× slower AND 2% larger

- **Priority:** P1 (worst S>1&T>1 class: no trade exists)
- **Score evidence:** rfc.txt q11 **I=12.7** (T=12.5×, S=1.020).
- **Status:** large-file half DONE 2026-09-08 (v0.21.70): bt and
  dict candidates gated to n <= 256 KiB (both never win above it in
  the corpus; bt explodes 4x on periodic). words q11 20.2s -> 8.1s
  user (T 4.7 -> 1.9), fits q11 same-load A/B 169.6s -> 70.0s,
  output byte-identical. The gated path still runs the hq a/b
  literal-assignment contest (csv2m 120,012 vs 173,007 — the split
  variant is essential). REMAINING (small files, this task's items
  1-2): noto q11 I=9.7, sqlite q11 I=8.7, rfc q11 I=8.0 — 4
  candidates + up to 7 emissions on 85-130 KB inputs. Plus the hq DP
  constant factor on big binary (fits q11 T=4.7 at the DP itself).

## Root cause

The q10/11 emission contest runs up to FOUR parse candidates
(hq, hq+dict, btopt, iterative), each measured twice (split a/b) plus
the tree-cap re-measure — ≥9 full emissions plus two DP passes on a
25 KB file, for a result that is still 2% larger than the reference.
The contest maximizes size only; it has no cost model for its own
time, and at small n the per-chunk fixed work dominates.

## Plan

1. **Cheap-candidate-first ordering with early exit**: measure the
   candidates in ascending cost order (iterative → btopt → hq →
   hq+dict). Stop when the best-so-far is smaller than the remaining
   candidates' realistic ceiling — simplest sound version: skip the
   hq/hq+dict DP passes entirely when the iterative candidate already
   measures within 1% of btopt (the DPs rarely recover that gap on
   small dictionary-dense text; measure the skip-rate on the corpus).
2. **One-shot emission per candidate** at n ≤ 64 KiB: at small n the
   split-variant b measurement wins <1% of the time — measure a only,
   unless a's margin to the previous candidate is <1% (then measure
   b). Halves the emissions on exactly the cells where overhead
   dominates.
3. **The S=1.020 residual** is the documented header-wire encoding
   gap (TODO.remaining/27 final decomposition — content bits at
   parity). Closing it is the header audit, NOT more candidates;
   this task must not add emissions chasing it.

## Acceptance

- rfc q11: T ≤ 4× with S unchanged (1.02) or better; every other q11
  cell byte-identical or smaller (the skip gates are conservative).
- Corpus sweep re-run; I re-scored for all q10/q11 cells.
