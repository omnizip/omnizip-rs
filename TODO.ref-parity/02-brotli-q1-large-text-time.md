# 02 — brotli q1: ship the reference's own fast tier

- **Priority:** P0
- **Score evidence (v4):** fits q1 **I=44.5** (T=58.5x, S=0.760) —
  the worst cell on the board. csv2m q1 I=34, words q1 I~19.6 (v3),
  plists q1 I=9.4, icons q1 I=6.6.
- **Status:** done 2026-09-08 (v0.21.68)

## Root cause (confirmed)

The task's original hypothesis (two-pass stored-block churn on
wordlists) was wrong: sampling showed q1 on 4 MB input routes to the
FROM-SPEC parse (`parse_input_with_offset_impl` +
`HashChainMatchFinder`) — the two-pass fragment compressor sat behind
the opt-in `BROTLI_TP` flag. The from-spec q1 was a deliberate
size choice (11-24% smaller than ref) that predates the inequality
lens: 5-58x slower for that size.

## Fix (v0.21.68): flip the q1 default to the two-pass

The two-pass fragment compressor is the transliterated reference
q1 (BrotliCompressBlockFast) — A/B measured it byte-exact with the
CLI on most content. Routing flipped to two-pass default;
from-spec stays behind `BROTLI_FS_Q1`.

Measured after the flip (ours vs ref, all 11 corpus files):
rfc I=0.3 (S 0.969), dbdump 1.3, words 1.5 (S 0.997 — BEATS ref),
rustsrc 1.7, csv2m 1.5 (S 1.000), **fits 1.3** (was 44.5),
noto 0.5, sqlite 0.8, plists 1.4, install 0.4, icons 0.9.
Every q1 cell I = 0.3-1.7; T = 0.3-1.7x; S = 0.969-1.000.

One-time output change at q1; regression baseline has no q1 rows.
Gates: brotli 104 tests, regression, property suites all green.

## Acceptance

- [x] words q1 I <= 3 (measured 1.5); every q1 cell I <= 1.7
- [x] CSV/FITS q1 cells: fits 44.5 -> 1.3, csv2m 34.1 -> 1.5
