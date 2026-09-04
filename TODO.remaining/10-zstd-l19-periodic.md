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

## Validated decomposition (2026-08-31, post Repeat_Mode fix)

The earlier attempt to dump ref-side SEQ_STATS at L18 produced
impossible numbers (6,139 seqs for 1.5 MB) — root cause: our decoder
rejected sequence-table Repeat_Mode, panicking mid-stream; the stats
came from a partial decode. Fixed in 0.21.32 (PR #421); reference
stats below are from a full, byte-verified decode.

| | ours L18 | ref L18 | delta |
|---|---|---|---|
| sequences | 233,746 | 282,595 | ours −17% |
| literals | 208,450 | 114,726 | ours **1.82x** |
| match bytes | 1,330,718 | 1,424,428 | −93,710 |
| avg match len | 5.69 | 5.04 | ours longer |
| max match len | 18 | 20 | |
| off > 127K | 55,818 | 58,981 | similar |

Confirms the qualitative read above, with a sharper shape: the
reference's gain is *more* sequences (not longer ones) — it converts
~94 KB of our literals into dense short matches (avg 5.0 B) while we
emit fewer, slightly longer matches. Same shape as the brotli
q9/q11 residuals. Any fix is a price-model change: lower the
effective price of short ml at large offset codes (or raise literal
price) — must sweep the whole corpus before shipping since the
other 9 files sit at 1.00-1.02x.

Interop note: decoder now decodes all 10 corpus files x ref levels
1-19 (190 streams) byte-identical; regression fixture
tests/fixtures/zstd/repeat-mode.zst.

## 2026-08-31 (0.21.33, PR #424): root cause was NOT the price model

Position-diff evidence: on a 128KB slice the parses are IDENTICAL
(19,638 vs 19,647 seqs, same literals/avg-ml) yet ours was 1.063x —
the whole gap was entropy-coding locality. The reference emits ~1KB
sub-blocks (43 per 128KB slice) with locally fitted tables reusing
each other via Repeat_Mode/Treeless; we emitted one monolithic
128KB block.

Ported: post-parse block splitting (C ZSTD_deriveBlockSplits —
recursive bisection on sequence indices, exact-measured sizes, gates
strategy>=btopt && windowLog>=17, <300 seqs no split, <=196 splits)
+ Repeat_Mode sequence-table emission + partition threading.
Result: csv2m L18 1.190x -> **1.093x**, L19 1.151x -> **1.046x**;
every other sweep cell within 1.000-1.013x or beating (arial L22
0.982x). 50 cells byte-verified by zstd -d.

### Remaining residual (small, documented)

- Reference still splits FINER (ref ~500 blocks on the full file vs
  our ~150) and reuses literal Huffman tables more aggressively.
- RLE-mode sequence tables unemitted: the C builds the single-state
  table outside buildCTable (ZSTD_buildSeqTable set_rle); our
  synthetic table_log=0 norm underflows build_ctable's delta math.
  Worth ~44 B on csv2m — needs the special-cased table if ever
  wanted.
- The earlier "price model" hypothesis is retired: parse parity on
  single blocks disproves it; the full-file seq-count difference
  (233K vs 282K) is a byproduct of the reference's own finer block
  boundaries, not a denser parser.

## CORRECTION (2026-09-01): the "parse parity" was a COUNT artifact — the price-model/parse question is REOPENED

Sequence-level alignment of the full-file L18 streams (ZSTD_SEQ_DUMP
on both, ours via our decoder): the parses diverge at output position
17 — the SECOND sequence (ours: ll=11 ml=3 off=4; ref: ll=16 ml=6
off=32) and crisscross constantly, re-syncing at shared positions.
The earlier slice experiment compared AGGREGATE counts (19,638 vs
19,647) and misread that as parse parity. Real shape: ours 233,746
seqs / 208K literals vs ref 282,595 seqs / 115K literals — the
reference converts ~94KB more literals into short matches (see the
python alignment: first-divergence dump in this task's history).

### Next attack (task-09 method, opt tier)

Position-by-position diff of our opt-tier CANDIDATE LIST + prices vs
zstd_opt.c's insertBtAndGetAllMatches at the first divergence
(position 33: ours had (4,3), ref took (32,6)). Two candidate causes:
(a) the tree/HC3 candidate generation never offers the short
recent-offset match (finder side — like task 09's missing table
policies), or (b) the price model rejects it (short-ml-at-offset-cost
pricing). (a) is the likelier and the more tractable; instrument
OMNIZIP-style dumps of insert_and_get_all_matches at a fixed position
and compare against the C's list shape.


## Closed 2026-09-04 — generator artifact

The 1.15-1.19x cells were measured on the hand-rolled synthetic csv2m
fixture. The exact in-tree-generator csv2m sweep (baseline.txt,
2026-09-03) shows zstd q19 at **0.925x** (152,845 vs ref 165,209 —
BEATS). Closed per the "always regenerate corpus fixtures from
in-tree generators" lesson.
