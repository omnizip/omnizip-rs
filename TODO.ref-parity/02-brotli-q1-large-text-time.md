# 02 — brotli q1 on ≥1 MiB text: 22× slower for 11% smaller

- **Priority:** P0
- **Score evidence:** words q1 **I=19.6** (T=22×, S=0.890). The old
  "q1 ✓ parity" standing was measured on 1–4 MB CSV/FITS; words
  (2.5 MB wordlist) shows the two-pass path is NOT at parity on this
  class.
- **Status:** pending

## Root cause (to confirm by profile)

q1 routes to `fast_encoder::compress_two_pass_q1` (the reference's
two-pass fragment compressor, transliterated and once optimized:
2026-08-21 fixed the unaligned-load/store primitives and BEAT the CLI
on CSV/FITS). Words is a different shape: a 2.5 MB degenerate
wordlist. Suspects, in order:
1. **GetHashTable sizing / stored-block fallback** — the two-pass
   emits stored blocks when the hash fills; wordlists churn the table.
2. **Block-scan cost at 1<<17 blocks on low-entropy input** — the
   second pass rebuilds the command tree per 128 KB block; 20 blocks
   × table build on 24 K-entry histograms.
3. The from-spec fallback path may be taken for this input class
   (check the routing: `BROTLI_NO_TP` A/B to see which path ships).

## Plan

1. A/B: `BROTLI_NO_TP=1` on words q1 — is the fallback faster/smaller?
2. Profile the two-pass on words (sample + force-frame-pointers); the
   2026-08-21 lesson says check the load/store primitives FIRST for
   this transliteration.
3. If the two-pass is fundamentally mismatched to wordlist entropy,
   route q1 large-text through the bank greedy with a size-shielded
   contest (greedy candidate measured; ships only if within 1% of the
   two-pass size — the I=19.6 cell wants the 22× back far more than
   the last 2% of the 11% size lead).

## Acceptance

- words q1: I ≤ 3; CSV/FITS q1 cells unchanged or better.
