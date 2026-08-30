# Task 10: Zstd L19 periodic-CSV cell (DEFERRED, re-diagnosed 2026-08-30)

## Status: deferred — re-confirmed with fresh evidence

## Fresh measurements (csv2m.bin, 1,539,192 B, current main)

| level | ours | ref | ratio |
|---|---|---|---|
| L16 | 204,674 | 213,740 | 0.958 ✓ |
| L17 | 204,674 | 222,029 | 0.922 ✓ |
| L18 | 192,844 | 162,094 | **1.190 ✗** |
| L19 | 193,911 | 168,529 | **1.151 ✗** |
| L20-22 | 193,911 | 168,529 | 1.151 ✗ (levels collapse) |

Broader than the original note ("L16-18 fine"): the gap spans
L18-22, exactly the minMatch-3 tiers. L19 across the whole sweep
corpus is 1.0000-1.0162 on the other 9 files — this cell is
periodic-CSV-specific.

## What was ruled out (2026-08-30)

- **optLevel**: the C's btultra AND btultra2 both run
  `ZSTD_compressBlock_opt2` (optLevel 2) — identical to ours; not a
  config divergence.
- **btultra2 two-pass seeding**: first-block-only, worth ~0.5% in the
  C's own comment; cannot explain 15%.
- **minMatch-3 path broken**: forcing minMatch 4 at L18 measures
  192,844 → 205,269 — our 3-byte path is net-POSITIVE (+12K), just
  weaker than the C's (+51K on its own L16→L18).
- **window/chain caps**: adjusted window_log = 22 covers the whole
  1.5 MB input at every level; the level collapse L20-22 is expected
  (srcSize-bounded).

## Actual cause (characterized)

csv2m is a quasi-periodic numeric CSV ("0,user_0,city_0,cc,...":
rows repeat in SHAPE, not bytes — no lag in 7K-700K matches cleanly;
only digit-pattern fragments repeat). The C's ultra DP converts
literals to dense 3-byte digit-fragment matches; our DP prefers
longer inserts and captures ~24% of that value. Same structural
family as the brotli q9/q11 residuals this session: the reference's
denser short-match parse. Tuning our DP toward it risks the nine
cells at 1.00-1.02x (every documented knob experiment in this area
regressed something else).

## Why still deferred

Degenerate synthetic fixture, one cell family, no downstream
trigger; L3-L17 and every other corpus at L18-22 meet or beat the
reference. The fix would be a DP price-model rework for short-match
density — measured globally before shipping.

## Triggers to revisit

- A downstream user reports periodic-structure data compressing
  worse at L18+ than mid levels
- A global re-tune of the opt price model for short matches (must
  sweep the whole corpus, not just this cell)
