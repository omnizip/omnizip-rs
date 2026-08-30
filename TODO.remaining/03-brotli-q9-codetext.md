# Task 03: Brotli q9 code-text gap diagnosis

## Status: done (2026-08-30) — emission-side fixed; parse-side residual documented

## Decomposition (DEC_STATS, ours vs reference q9 on rustsrc.txt)

ours 364,808 vs ref 340,647 (1.0710x), 227K-bit entropy gap:

| component | ours | ref | delta (bits) |
|---|---|---|---|
| cmds | 120,705 | 103,596 | — |
| literals | 56,521 | 70,783 | — |
| avg_copy | 31.0 | 36.0 | — |
| cmd_sym | 608,152 | 532,984 | +75.2K |
| lit_sym | 296,459 | 336,520 | −40.1K (win) |
| dist_sym | 611,477 | 504,262 | **+107.2K** |
| dist_extra | 1,242,305 | 1,150,665 | **+91.6K** |
| dist trees | 4 | 50 | — |
| cmd blocks | 16 | 3 | — |
| lit trees | 13 | 238 | — |

**88% of the gap is distance-side** (dist_sym + dist_extra).

## Emission-side fix (shipped): free-clustering dist split at q5+

The reference distance block splitter (1.2.0 SplitByteVector, free
histogram clustering) was gated q10+; q4-9 ran the in-house DP with
hard caps (4 blocks / 4 trees). Now q5+ uses the reference splitter
with scaled tree counts (`shared_k = (nb_d*4).clamp(2,32)` instead of
`.min(4)`), matching the q10+ path.

Measured (all reference-decode verified):

- rustsrc q9 364,808 → 363,596 (1.0710 → 1.0673x)
- FITS q9 2,387,194 → 2,381,284 (still beats ref)
- csv-real q5 433,777 → 433,295; q9 430,911 → 430,209
- csv100k q5 10,119 → 9,992; q9 9,479 → 9,426; text/binary 100KB
  identical (regression baseline refreshed)

## Parse-side residual (documented, not fixed)

Our greedy parse over-copies relative to ref's lazy: 17K more
commands, avg_copy 31 vs 36, +17K explicit distances at ~0.77 extra
bits each (dist_extra). The distance distribution is flat (top
explicit distance used only ~300 times of 119.5K) — the reference's
lazy matcher concentrates on fewer, longer, nearer copies. Closing
this needs the reference's q4-9 lazy+HQ-hasher engineering — the same
structural item the tier-flip campaign documented as the deliberate
ratio/time tradeoff; our greedy wins CSV/FITS but loses code text.
Also related: ref's 238 literal trees (ContextBlockSplitter) vs our
13 — same clustering-port family as task 04.

## Acceptance

- [x] Root cause identified and documented (decomposition table)
- [x] Actionable part implemented, tested, shipped (q5+ dist split)
- [x] Residual documented with evidence (parse-shape, not emission)
