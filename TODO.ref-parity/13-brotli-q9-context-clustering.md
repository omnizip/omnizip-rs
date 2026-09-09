# 13 — noto q9 (I=4.4): context clustering + the neutral-rewrite pattern

- **Priority:** P2
- **Score evidence (v14):** noto brotli q9 **I=4.4 (T=4.3x, S=1.031)**
  — 0.032s user for 85 KB.
- **Status:** investigated 2026-09-09; two candidate fixes measured
  NEUTRAL. No code shipped. The analysis:

## Profile (6,443 samples)

Emission ~66% (metablock_from_commands 4,304 — of which
cluster_contexts ~4,457 with its greedy helper), bank parse ~33%
(scan_with_key + find_insert).

## Negative results (2026-09-09)

1. **Flat distance matrix for cluster_contexts_greedy** (the
   Vec<Vec<u64>> form pointer-chased rows in the per-merge O(m^2)
   scan and paid an O(m) memmove per row on removal): rewritten with
   an n-stride flat matrix + in-place compaction. Byte-identical —
   verified by a 300-case differential test against a verbatim copy
   of the original (the test CAUGHT a real bug during development:
   sequential packing broke the n-stride indexing the scan uses) —
   but performance-NEUTRAL on noto/sqlite/rfc/plists/words q9.
2. Same-day negatives on the H10 tree compare (task 12): three
   forms measured; the compare is not that loop's lever either.

## Pattern (recorded for the next pass)

Three consecutive "attributed-hot-loop + obvious safe rewrite" plays
measured neutral: profile attribution (sample self-time in inlined
frames) is NOT sufficient evidence that a specific rewrite will pay
— the cost apparently sits in cache misses shared by all forms.
Before the next rewrite attempt on these cells: instrument counts
(node visits, L1 calls) or accept the cells as
constant-factor-floor. The differential-test harness pattern
(verbatim original copy + 300 random cases) is the cheap insurance
that made the neutral outcome safe — reuse it for any future
equivalence-sensitive rewrite.

## Acceptance

- none yet; reopen if the I calculus changes or an instrumented
  probe identifies a count-level difference from the reference.
