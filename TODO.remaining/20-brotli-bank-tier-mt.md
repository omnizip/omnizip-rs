# 20 — brotli q4-9 bank-tier multi-threading

- **Priority:** MEDIUM (q5 4.7–6.1x, q9-binary within bar)
- **Depends on:** [13](13-encode-speed-parallelism.md), PR #467's chunk-MT shape
- **Estimated effort:** 2 days
- **Status:** closed 2026-09-04 — byte-identical infeasible (code-evidenced)

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


## Resolution (2026-09-04) — NOT feasible byte-identically

Both design questions answered in code:

1. **No store-only priming can reproduce the bank state.**
   `BankMatchFinder::insert` places a position in bucket slot
   `num[key] & mask` — the per-key insert COUNT decides slot
   placement — and the greedy loop's RLE guard
   (`distance < advance >> 2 → bank.skip()`, from_spec_encoder's
   greedy advance) makes the insert SET a function of the parse
   (chosen match distances), not of the input. A per-worker bank
   primed over the preceding window without the previous chunks'
   parses holds different bucket contents AND different slot
   placement — candidates, tie order, and therefore output differ.
2. The rep-ring itself is chunk-fresh (emission resets it), so the
   bank is the only blocker.

Alternatives, deliberately not taken:

- Insert-every-position store set (parse-independent) would make bank
  MT byte-identical AFTER a one-time change to every q4-9 output,
  plus the RLE hash-poisoning guard's ratio/time risk on repetitive
  content. That is an owner decision on a shipped-output change, not
  a perf PR.
- An opt-in non-identical MT path (zstd `compress_mt` style) has no
  API surface here (brotli rides the single `Codec::compress`) and
  defaults must stay byte-identical.

q4-9 stay sequential; the zopfli-tier MT (q10-11, and q9-text) covers
the tiers where parallelism is free.
