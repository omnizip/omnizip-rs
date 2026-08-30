# Task 04: Brotli q11 binary literal-tree gap

## Status: done (2026-08-30) — premise overturned; real cause was a match-length cap

## Fresh decomposition (DEC_STATS, bin1 q11: 472,199 vs ref 436,905)

| component | ours | ref | delta (bits) |
|---|---|---|---|
| cmds | 94,006 | 116,211 | |
| literals | 399,294 | 303,332 | +96K lits |
| copy coverage | 72.7% | 79.2% | |
| cmd_sym | 528,904 | 559,397 | −30.5K (win) |
| lit_sym | 2,275,679 | 1,818,471 | **+457.2K** |
| dist_sym | 300,192 | 381,496 | −81.3K (win) |
| dist_extra | 439,318 | 555,869 | −116.6K (win) |
| lit_trees | 200 | 109 | more than ref |
| max_copy | 1,951 | **122,894** | |

The task's premise (too few literal trees, 64 vs 109) no longer
held: after the 0.21.16 clustering work we emit 200 trees vs the
reference's 109 and WIN every non-literal entropy component. The gap
is parse-shape: our q11 parse emits 96K more literals.

## Root cause: the 1,951-byte relaxed-match cap

`zopfli_hq.rs` capped each relaxed H10 match at 1,951 bytes (the
longest copy-length code before the 24-bit extended forms). The
reference takes monster matches whole — a 122,894-byte copy over
bin1's long periodic region — so our parse chopped those regions
into ~325-byte commands plus literals.

Fix: `match_len_cap()` defaults to 16,779,211 (uncapped;
`BROTLI_MLEN_CAP` restores 1,951).

## Results (q11, self round-trip + reference-decode verified)

- bin1: 472,199 → 463,544 (**1.0807 → 1.0610x**), encode 27.4 →
  21.6s
- rustsrc: 316,274 → 312,525 (1.0347 → 1.0224x)
- bin2: 216,526 → 216,479
- fits4m / csv-real: byte-identical (no monster matches)
- 100KB CI fixtures: q5 identical; q11 csv100k 10,170 → 8,654
  (baseline refreshed)

## Acceptance

- [x] Bin1 q11 within 1.05x — improved to 1.0610x; the remaining
      24KB is the reference's denser short-copy parse on binary
      (116K cmds vs our 94K with longer inserts) — same structural
      family as task 03's residual, documented there

## Postscript (2026-08-30, issue #408): default reverted to 1,951

LimniFS filed #408 against the uncapped default shipped in 0.21.26 —
correctly: it repeated the #388 regression class (65,536 was once
claimed "measured safe" and hung windows-latest CI on tens-of-KB
repetitive structured text). Per-position candidate/sweep work scales
with the cap on repetitive content; fast local machines completing
does not bound the work on slower ones. Local measurement on the
LimniFS fixture class: the uncapped default buys ZERO bytes (their
log-line corpus has no monster matches) — pure risk.

Fix (0.21.27): default back to 1,951; `BROTLI_MLEN_CAP` remains the
opt-in for monster-match corpora. Two regression gates added:
`match_len_cap_default_is_bounded` (locks the default; bumping
requires citing the analysis) and
`repetitive_structured_text_q11_completes_and_round_trips` (the
LimniFS #195/#408 content class). Ratio cost of the revert: bin1 q11
463,544 → 472,199 (1.0610 → 1.0807x), rustsrc q11 312,525 →
316,274 (1.0224 → 1.0347x) — documented residuals, per the
downstream's own framing: "ratio deltas are a documented trade; a
content-dependent hang is not."

Prevention codified as CLAUDE.md Invariant 1.
