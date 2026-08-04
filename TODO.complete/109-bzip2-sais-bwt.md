# 109 — BZip2 SA-IS suffix array for faster BWT

**Priority:** P2 — MEDIUM
**Source:** Performance audit (2026-08-04)
**Status:** ⏳ Pending

## Problem

`bwt.rs` uses Manber-Myers prefix doubling with `sort_by` per iteration:
O(n log² n) comparisons, each O(1) amortised. For 900KB blocks this is
2-5x slower than reference bzip2, which uses a divsufsort-inspired
approach.

**Impact:** 2-5x BWT construction slowdown. Doesn't affect ratio.

## Proposed fix

Replace `build_suffix_array` with SA-IS (Suffix Array Induced Sorting):
- O(n) construction time
- Recursive induced-sort algorithm
- Pure Rust implementations exist (e.g., in the `sais` crate pattern)

Alternatively: port divsufsort-lite's two-stage approach.

## Acceptance criteria

- [ ] SA-IS or divsufsort implementation in `bwt.rs`
- [ ] BWT round-trip preserved (all bzip2 tests pass)
- [ ] 900KB block BWT construction < 1 second (currently ~3-5 seconds)

## Effort estimate

2-3 days.
