# Task 16 — brotli: skip the b (split literal assignment) emission on binary inputs

Status: done (2026-09-12, shipped v0.21.82)
Parent: task 04 (contest overhead), task 10 (splitter time)

## Problem

The q10/11 reduced contest (all inputs that are NOT small+dense — i.e. every
large file) runs **three full emission measurements** per chunk: a (decided
static map), b (`with_lit_split_override(true)` split assignment), treecap
(cap-6 literal clustering). The C reference runs one parse + one emission.
On binary content the b measurement never wins — it is pure overhead. fits
q11 was the worst board cell (I=4.9, T=66.4s vs ref 13.5s) with the splitter
work (population_cost_pair 46% self, find_blocks, histograms) living inside
those three emissions.

## Investigation (2026-09-12)

Added `BROTLI_BTOPT_DUMP` instrumentation to the reduced path (it previously
only dumped the full contest) and a `BROTLI_NO_SPLITCAND` timing knob.
Sweep, q11 (single chunk each unless noted):

| file              | class      | a (bits)  | winner    | margin      |
|-------------------|------------|-----------|-----------|-------------|
| fits4m.bin        | Binary     | 11,893,706| a         | —           |
| csv2m.bin         | Structured | 1,379,083 | **b**     | **−31.0%**  |
| words.txt         | Text       | 5,216,518 | a         | —           |
| rustsrc.txt       | Text       | 3,027,107 | **b**     | −0.25%      |
| rfc.txt           | Text       | (4 chunks, full contest path) | | |
| dbdump.txt        | Text       | 426,443   | a         | —           |
| noto-otf.bin      | Binary     | 653,500   | treecap   | −2.5%       |
| sqlite.db         | Binary     | (4 chunks, full contest path) | | |
| plists.json       | Structured | 926,966   | **b**     | −7.4%       |
| install.log       | Text       | 120,601   | treecap   | −0.8%       |
| icons.svg         | Text       | 211,218   | treecap   | −0.9%       |

q10 binary winners (via `BROTLI_SPLITCAND_ALL=1` + dump): fits → a, noto →
treecap (b loses); sqlite takes the full-contest path (ungated).

Every measured b win is `ContentType::Text | Structured`
(`is_text_like`); every binary input loses b. This is the same clean split
the `run_bt` gate already uses.

## Change

`omnizip-brotli/src/from_spec_encoder.rs`, reduced contest path:

- `run_b = (is_text_like(input) || BROTLI_SPLITCAND_ALL) &&
  !BROTLI_NO_SPLITCAND` — b is only measured on text/structured inputs.
- Winner dump line added (`BTOPT chunk@.. n=.. a=.. win={a|b|treecap}@..`).

Treecap stays unconditional: it wins on both binary (noto −2.5%) and text
(install/icons) — no clean class gate. (The A/B/C literal-assignment contest
inside `measure_emission_bits` explains the cap-insensitive results: treecap
only affects path A; on fits/words/dbdump B/C won, making the cap moot.)

## Verification

- 11-file corpus q11: outputs **byte-identical** to v0.21.81 default.
- fits/noto/sqlite q10: byte-identical (gate vs `BROTLI_SPLITCAND_ALL=1`).
- Timing: fits q11 66.44s → 46–48s (**−28…30%**); noto q11 1.18→1.00s;
  sqlite/words/csv2m/rustsrc/plists unchanged (text class keeps b).
- Gates: 105 tests, fmt clean, clippy (CI invocation), regression board
  sweep re-scored → v19.

## Residual

- words/dbdump/install/icons (text class) still pay b when it loses there —
  no clean predictor separates csv2m/rustsrc/plists (b wins) from
  words/dbdump (b loses) within text. Accepted insurance premium.
- fits q11 T ≈ 3.4 remains the worst cell: remaining time = hq parse (2 DP
  passes) + 2 emissions (a + treecap). Next lever would be a cheaper
  treecap (reuse non-literal emission parts) — measured-out for now.
