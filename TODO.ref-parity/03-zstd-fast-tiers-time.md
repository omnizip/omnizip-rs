# 03 — zstd fast/intermediate tiers: 20-30x slower for a few % smaller

- **Priority:** P0 (biggest cluster on the board)
- **Score evidence (v3):** 10 cells at I=17-29, all zstd L6 (T=18-30x,
  S=0.87-0.99). csv2m zstd L6 I=137 (worst cell, pre-getenv-fix).
- **Status:** lazy/lazy2 band SHIPPED (v0.21.67); dfast (L3-4) and
  btlazy2 (L13-15) still on the opt DP — see "Remaining".

## Root cause (confirmed)

block.rs routed ALL strategies >= DoubleFast — every level 3-19 —
into the opt price-DP parser: a deliberate size-parity choice that
predates the inequality lens. The reference runs greedy (L5), lazy
(L6-7), lazy2 (L8-12) hash-chain parses at those levels: O(n) with
small constants. The old hash-chain lazy port had plateaued at
1.07-2.45x size because it lacked repcode probing, backward
catch-up, real lookahead search, and ran chain-less in cross-block
mode (its chain table was block-local, so it was disabled there).

## Fix (v0.21.67): reference-shaped lazy parser

`omnizip-zstd/src/encoder/lazy.rs` — a line-faithful port of
`ZSTD_compressBlock_lazy_generic` + `ZSTD_HcFindBestMatch` +
`ZSTD_insertAndFindFirstIndex_internal` (zstd_lazy.c, noDict):

- window-masked chain table (`1 << chainLog`, absolute positions,
  persists across blocks — `MatchState::enable_hc`)
- repcode checks: rep0-at-ip+1 baseline, rep-in-depth-loop with the
  C's gain formulas (`ml*3/4 - highbit32(offBase)`), post-store
  immediate-repcode loop with rep swap
- backward catch-up (extend matches over literals)
- step acceleration + lazySkipping over incompressible runs
- depth 0/1/2 = greedy/lazy/lazy2

Routing: L5 (Greedy) -> depth 0, L6-7 (Lazy) -> depth 1, L8-12
(Lazy2) -> depth 2. DoubleFast (L3-4) and Btlazy2 (L13-15) stay on
the opt DP (dfast/btlazy2 parse shapes unported — the DP is
size-superior there and the board has no L3-4/L13-15 whack cells).
Dictionary path unchanged (legacy parsers; its own parity task).

### Measured (2026-09-08, box load ~34 — times inflated)

Sizes ours/ref at L6 across the 11-file board corpus:
rfc 1.0002, dbdump 0.9939, words 1.0039, rustsrc 1.0093,
csv2m 0.9722, fits4m 0.9489, noto 0.9996, sqlite 0.9917,
plists 0.9949, install 0.9991, icons 1.0045 — S=0.949..1.009
(acceptance was S <= 1.02). Band spot checks: L5 1.004, L7 0.98-1.003,
L9 0.977-1.003, L12 0.988-1.002.

Times (RUNS=10 amortized, load ~34): csv2m L6 3.024s -> 0.065s
(46x), words 0.688 -> 0.129 (5.3x), rustsrc 0.640 -> 0.074 (8.7x).
Estimated L6 board cells fall from I=17-29 to I ~ 3-4.5.

Regression-gate rows that moved (zstd/l9 csv_100k +41%, binary_100k
+54%, text_100k +1.2%) were re-checked against the reference: ours
now 18,569/1,096/173 vs ref 18,874/1,106/174 — the new output BEATS
the reference on all three; the old numbers were the DP
overachieving at 20-30x time cost. Baseline refreshed (one-time tier
change).

## Remaining

- **dfast (L3-4)**: port `ZSTD_compressDoubleFast` (two hash tables)
  if L3/L4 cells ever reach the whack list.
- **btlazy2 (L13-15)**: binary-tree lazy2; opt DP currently
  size-superior — leave until measured otherwise.
- **Dictionary path**: route through the new parser (needs dict chain
  seeding; changes dict output).
- The old block-local `compress_block_lazy`/`lazy2` remain for the
  dict path only.

## Acceptance

- [x] L5-12 sizes within 1.02x of reference (measured 0.949..1.009)
- [x] L6 times: 5-46x faster than the old opt routing; est. T ~ 3-4x
- [x] Round-trip + bounded-work fixtures (zeros, 7,000-period
      periodic) green; regression baseline refreshed
- [ ] Quiet-box v4 board numbers in README (deferred with task 05's)
