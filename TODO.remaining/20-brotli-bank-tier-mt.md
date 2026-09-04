# 20 — brotli q4-9 bank-tier multi-threading

- **Priority:** MEDIUM (q5 4.7–6.1x, q9-binary within bar)
- **Depends on:** [13](13-encode-speed-parallelism.md), PR #467's chunk-MT shape
- **Estimated effort:** 2 days
- **Status:** pending 2026-09-04

## Goal

Extend PR #467's byte-identical chunk MT from the zopfli tiers
(q10-11) to the bank tier (q4-9), where `BankMatchFinder` state is the
cross-chunk dependency to reproduce.

## Design questions to answer first (in code)

1. Does `BankMatchFinder` have a store-only path equivalent to
   `prime_until`? The bank inserts every position as it scans
   (`find_insert`); a per-worker bank over the full input primed
   store-only through the preceding window must reproduce the shared
   bank's bucket contents. Check whether bucket eviction (circular
   slot reuse) depends on scan order beyond position order — if
   eviction is per-position deterministic, priming works; if it
   depends on probe counts, it does not.
2. The greedy/lazy parse's rep-ring probing reads `last_dists` state
   carried across chunks (mf_base threading, PR #300/#301) — does
   chunk N's parse start from a deterministic rep state derivable
   from input only (like the zopfli tier's forced reset), or from
   chunk N−1's parse output? The latter breaks byte-identical MT;
   options: same reset-at-chunk-start trick, or measure the ratio
   delta of resetting.
3. q9 text runs the DP tier (28–56x cells) — routing note: q9-text
   MT gives ≤4x; the residual single-thread gap is task 21.

## Acceptance

- Byte-identity vs `BROTLI_NO_MT=1` on words/rustsrc/csv21m/fits4m at
  q5 and q9 (both content classes), all REF-OK.
- Determinism across worker counts.
- Measured speedups recorded here.
