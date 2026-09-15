# Task 39 — brotli: treecap reachability margin (skip the third emission on runaway gaps)

Status: done (2026-09-15, shipped v0.21.94)
Board cells: csv2m brotli q11 I 2.6→**1.27** (under the 1.3 bar); fresh
quiet-load re-score of the whole q11 column (board v36)

## Finding

Task 37 skipped the treecap re-measure when the assignment winner was
cap-INSENSITIVE. csv2m's a-emission is cap-sensitive (cmap_a wins it),
so treecap still ran as a third emission — even though csv2m's contest
is decided by 31% (the b-split variant wins): a tree-cap map moves an
emission by at most ~5.3% corpus-wide (plists, measured), so a config
8%+ behind the winner can never be rescued by the cap.

## Change

`omnizip-brotli/src/from_spec_encoder.rs`: both treecap sites now also
require `a_bits <= win_bits + win_bits/12` (≈8.3% margin, 3% headroom
over the largest measured cap effect). `BROTLI_TREECAP_ALL` restores.

## Verification

- q10/q11 × 11 files: **22/22 byte-identical** to v0.21.93.
- Fresh quiet-load (8) q11 re-score vs the reference CLI:
  csv2m T 1.61 × S 0.789 = **I 1.27**; rustsrc 1.32×1.010 = 1.33;
  plists 1.36×1.023 = 1.39; sqlite 2.33×0.988 = 2.30; noto
  1.95×1.005 = 1.96; dbdump 1.10; fits 0.59; words 1.18.
- ALSO CLOSED (task 38 verification gap): task 38's sweep covered
  q5/q9/q11 but not q10 — measured here: fits q10 −0.31%, csv2m q10
  **−14.6%** (hier found a much better clustering), noto −0.064%,
  plists/sqlite +0.006-0.009%, rest identical; all round-trip.
- Gates: 104+1 tests, fmt.

## Residual (>1.3 on the brotli side)

sqlite q11 (2.30) and noto q11 (1.96): both run exactly two emissions
(a + treecap) where treecap WINS (legit) — their residual is the hq
parse + double emission at the per-op floor. rfc q11 (~2.0-2.5,
sub-100ms class): same. rustsrc (1.33)/plists (1.39): at the line.
