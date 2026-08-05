# TODO 163: GLZA O(N²) cap — linear-time path

## Problem

`omnizip-glza` is registered but currently caps input size to
avoid O(N²) blowup in the grammar-construction phase. Real text
inputs above the cap get rejected.

## Scope

GLZA (Grammar-based LZ compression) builds a context-free grammar
representing the input. The current implementation has quadratic
cost in the grammar-inference step.

A linear-time path requires:
1. Suffix-array-based repeat detection (O(N log N) construction).
2. Hash-bucketed candidate generation (O(N) per pass).
3. Bounded rule-explosion via greedy longest-match replacement.

## Implementation plan

1. Replace the current O(N²) repeat-finder with a suffix-array
   version.
2. Cap grammar rule count to bound memory + encode time.
3. Verify ratio doesn't regress on benchmarks.

## Acceptance criteria

- [ ] No input-size cap on `GlzaCodec::compress`.
- [ ] 1 MiB text input completes in < 5 s.
- [ ] Ratio within 5% of the current capped version on inputs
  below the cap.

## Priority

P2 — research codec; not on the LimniFS critical path.
