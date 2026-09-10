# 04 — brotli q11 small-file contest: 12.5× slower AND 2% larger

- **Priority:** P1 (worst S>1&T>1 class: no trade exists)
- **Score evidence:** rfc.txt q11 **I=12.7** (T=12.5×, S=1.020).
- **Status:** large-file gating DONE (v0.21.70); content-class
  gating DONE 2026-09-08 (v0.21.71). Remaining: rfc-class cells and
  the hq DP constant factor — see "Remaining".

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

## Content-class gating (v0.21.71)

BTOPT_DUMP on the small-file cells: on dictionary-SPARSE inputs (the
density screen's 0.00-0.03 class — noto, and everything large) the
hq parse + tree-cap refinement wins every contest (noto:
hq=657,787 bt=666,426 hqdict=660,316 iter=709,358 -> treecap 641,823
ships); on dense TEXT the dict candidate wins (rfc: hqdict=56,050 ->
treecap 53,418); on dense BINARY-text (sqlite, whose DB strings pass
the screen) bt loses everything (359,318 vs hq 336,699).

Shipped: (a) all three extra candidates (bt/hqdict/iter) now run only
when the 512-sample density screen says dense; (b) the sparse path
(hq + a/b + tree-cap) got the tree-cap refinement it was missing —
without it noto regressed +2.5% (82,224 vs 80,229: the cap is where
the sparse class's size comes from); (c) bt additionally requires
is_text_like (sqlite/noto false, rfc/plists/install true).

Measured (user time): noto q11 T 9.6 -> 3.9, sqlite 8.7 -> 5.5.
Output byte-identical on every cell.

## Remaining

- **rfc q11 (I=8.0, T=7.8x, S=1.020)**: dense text keeps the full
  contest by design (its winner IS a gated-on candidate). Levers:
  conditional split-b (the b variant won in ZERO of 8 measured
  contests — measure b only for the winner / when margins < 1%),
  and the header-wire S residual (separate audit, not this task).
- **hq DP constant**: fits q11 T=4.7 and rfc-class T are dominated by
  the zopfli_hq port's per-node cost (~3-4x the reference DP). A
  profiling pass over encoder/zopfli_hq.rs is the next sizeable win.

## rustsrc q11 note (2026-09-09)

The v3-era size row (373,844) was stale: since the v0.21.70/.71
candidate gating, rustsrc q11 emits 379,830 (+1.6%) — a dict/bt
candidate had been winning that margin on 723 KB dense text. The
trade is I-improving per the board principle (the candidates cost a
DP + two emissions; rustsrc q11 T was ~5 at gating time and is 1.2
today). Recorded in the v14 board; do not chase the 1.6% with
candidate restoration unless the I calculus changes.

## Conditional split-b (v0.21.78, plan item 2)

Every candidate was measured under BOTH literal assignments before
the winner was known, though b lost every observed contest (8+
files). Now: a for all candidates; b only for the best (always) and
the runner-up within 1% (b's observed effect <2%, so a wider gap
cannot flip). Byte-identical on the corpus. rfc q11 T 5.4 -> 4.2
(I 5.5 -> 4.3 — was the board's worst cell), sqlite 3.7 -> 3.0,
plists -10%. Note the contrast with the same-day rewrite attempts
(tasks 12/13): removing work pays; reshaping attributed-hot loops
measured neutral three times.

## The S residual — decomposed and half-closed (v0.21.79)

The audit ran (2026-09-10): a new BROTLI_STRUCT_DUMP decoder stage
dump localized rfc q11's S=1.020 EXACTLY — same metablock shape
(6 lit trees, 2 dist trees, same nbltypes, our context maps SMALLER
by 19 bits, our payload 427 bits BETTER), but tree descriptions
2.1x the reference's (littrees 1,787 vs 836 bits; cmdtrees 1,010 vs
557) = the entire 130-byte gap.

- Root cause 1, FIXED (v0.21.79): build_rle_sequence's separator
  literals + untrimmed zero tails. Reference RLE ported verbatim.
  rfc q11 S 1.020 -> 1.0136; install q11 S 1.067 -> 1.0329; strict
  size win on every measured cell.
- Root cause 2, open (content-level): the remaining ~90 bytes are
  tree-SHAPE differences — our context clustering produces length
  distributions with more RLE runs than the reference's trees. That
  is the clustering's trade (our payload is better by more than the
  tree excess on rfc), not a wire defect. Closing it means trading
  payload entropy for tree simplicity — a contest-shaped change
  (add a clustering candidate whose trees measure smaller total),
  viable follow-up if S<1.01 ever matters.
