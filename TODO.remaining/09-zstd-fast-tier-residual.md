# Task 09: Zstd L1/L2 fast-tier residual cells (DEFERRED)

## Status: deferred

## Problem

zstd L1/L2 sit at 1.02-1.11x on most corpora, with L2-regcsv at 1.279x. The gap is in the fast-loop hash-table state management — the pipeline positions, hash writes, and step skipping form a coupled system where single-element patches regress (measured: adding the C's current0+2 fill made L2-regcsv WORSE).

## Why deferred

Closing this requires a complete line-by-line port of zstd_fast.c's 4-position pipeline including all goto-flow and hash-write ordering. The remaining gap is 2-11% on a fast tier where the C is already 20-30x faster — the cost-benefit is poor.

## Triggers to revisit

- zstd releases a new fast-loop design
- A downstream user reports a specific corpus with >1.3x gap at L1/L2

## Reopened 2026-08-31: the >1.3x trigger fired; gap decomposed

Fresh full-file matrix (10-corpus sweep, current main): rustsrc L2 =
1,018,408 vs ref 746,025 = **1.3651x** — the task's own reopen
trigger ("downstream reports a specific corpus with >1.3x at L1/L2")
crossed. Full L1/L2 picture: rustsrc 1.099/1.365, bin1 1.110/1.127,
words 1.045/1.055, arial 1.001/1.014; everything L6+ ≤ 1.005 (L6
beats ref nearly everywhere).

### Decomposition (new SEQ_STATS decoder dump vs FAST_STATS encoder dump)

| | ours L2 | ref L2 | delta |
|---|---|---|---|
| sequences | 262,384 | 219,390 | +20% |
| literals | 485,722 | 239,301 | **2.03x** |
| match bytes | 3,311,730 | 3,558,022 | −246K |
| coverage | 87.1% | 93.7% | −6.6pt |
| avg match | 12.6 | 16.2 | |

The 272KB output gap is fully explained: 246K bytes that the
reference matches, we emit as literals, plus 43K extra short-match
sequences. Same shape as the brotli q9/q11 residual — the reference
parser converts literals to matches more aggressively than ours.

### Ruled out this pass

- minMatch not plumbed — it is (mm 7→6 reaches fast4; parses differ)
- hash width primes — identical to C (5/6/7-byte primes, verbatim)
- hash_log not flowing — MatchState::new(params.hash_log) ✓
- step acceleration — kStepIncr/nextStep ported ✓
- 4-byte acceptance — same as the C's ZSTD_match4Found (mls only
  keys the hash)
- block size — 127KB ✓; the adaptive 16KB splitter fires on
  rustsrc (51 blocks vs ref's 31) worth ~15-20KB of headers, not
  the main gap

### Where the gap actually is

Candidate freshness / match-length yield inside the 4-position
pipeline on heterogeneous text — the coupled system this task
deferred. The next attack needs table-update-policy and rep-ride
diffing against zstd_fast.c position-by-position on a small
rustsrc slice; the two new env-gated diagnostics
(ZSTD_SEQ_STATS on any stream, ZSTD_FAST_STATS on encode) make
that tractable.
