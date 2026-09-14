# Task 32 — intermediate-level sweep (gate 3): 154 cells, clean bill

Status: done (2026-09-14; measurement only)
Trigger: "proceed with all 3" — new-corpus/contract evidence via a full
intermediate-level sweep the 3-level board never covered (canon raw
harness zlvl_tmp per task 31; brotli via gen_any_tmp).

## Coverage

brotli {2,3,4,6,7,8,10} + zstd {2,3,4,5,7,9,12} × 11 corpus files =
154 cells, sizes + `brotli -d`/`zstd -d` round-trips.

## Findings

1. **All 154 cells decode byte-exact. No routing anomalies.**
2. **zstd L4 rides the opt DP and wins hugely**: words L4 = 672,992 vs
   ref 854,292 (**−21%**). L3/L4 (DoubleFast per cparams) share the opt
   parse — a deliberate task-03-era choice, now verified at every file.
3. **zstd L5 (Greedy tier) sits at parity** (words 802,121 vs ref
   798,720, +0.4%) — the local non-monotonicity (L4 better than L5)
   mirrors the reference's own flat 3–5 band. Not a bug.
4. **brotli q6==q7==q8 byte-identical** (742,445 on words): the
   quality-config bands (4..=5, 6..=7, 8..=9) plus the bank-slot cap
   collapse the band's output — and still beat the reference's
   differentiated q8 (758,781) by 2.2%. Banding loses ≤0.6% of
   potential at q7/q8; a documented trade.
5. No level regressed vs its neighbors where the reference improves —
   every intermediate level is ref-competitive or ref-beating.

## Standing

The board's 3-level columns fairly represent their bands. The
intermediate levels need no separate tracking unless a downstream
contract names a specific level.
