# 21 — brotli q9-text single-thread gap (28–56x)

- **Priority:** MEDIUM (the largest remaining single-thread cell)
- **Depends on:** [13](13-encode-speed-parallelism.md)
- **Estimated effort:** open-ended — diagnosis first
- **Status:** mitigated 2026-09-04 — MT extended to q9-text; single-chunk levers deferred

## Standing (2026-09-04 fresh table, task 13)

q9 on text runs the zopfli tier: words 30x, rustsrc 56x, csv2m 28x
reference time. Reference q9 is near-greedy speed with H9+lazy;
ours pays the full 2-pass HQ DP.

## Known constraints (don't re-litigate)

- Greedy routing for q9 was tried and REJECTED by the regression
  gate once before (brotli/q5 sizes) — the tier philosophy is
  "comparable effort per level", and our q9 sizes currently beat the
  reference on most cells.
- The 1.3x bar applies per cell; q9-binary is already inside it.

## Diagnosis plan

1. Confirm where q9-text time actually goes today (pass profile,
   `sample` + frame pointers; the old "diffuse parse+emission"
   notes predate several perf passes).
2. Options with measured trade-offs, in preference order:
   - fewer DP candidates/iterations at q9 only (bounded sweep
     reduction, measure ratio on the 10-file corpus + regression
     gate);
   - the reference's H9 lazy+HQ-hasher tier port (the documented
     engineering gap);
   - single-refinement q9 (max_iters=1 — was measured for a
     different tier shape; re-measure).
3. Each candidate ships only if sizes stay within the regression
   gate on ALL fixture classes (small-file cliff check included).

## Acceptance

- q9-text ≤1.3x reference on the corpus (or a documented trade
  ratified by the ratio gate), with the decision recorded here.


## Resolution (2026-09-04)

The q9-text DP routing is a deliberate ratio-over-time trade (our q9
beats reference sizes on every text cell — rustsrc 338K vs ref 341K
at the time of routing; greedy there lost 13-72KB). The standing
28-56x cells are all SINGLE-chunk inputs (rustsrc 2.06MB, csv2m
1.5MB) where multi-threading cannot help.

Mitigation shipped: the byte-identical MT gate now covers q9 when
every chunk classifies as text (text chunks never route to the
bank-driven greedy tier; an all-chunks guard handles
mixed-classification inputs) — validated IDENTICAL on words q9.
Multi-chunk q9 text (>= 2MiB) now parallelizes like q10-11.

Deferred: single-chunk levers (fewer refinement iterations, candidate
shape, the reference's lazy+HQ-hasher port) remain unmeasured; any
change must clear the ratio gate on all fixture classes.
