# Task 23 — zstd: interleaved histogram + unstable sort (L1 remainder)

Status: done (2026-09-13, shipped v0.21.86)
Parent: task 22 (post-.85 profile follow-up)

## Profile (fits L1 post-v0.21.85, RUNS=300, load ~6)

Post-two-queue, `build_weights` was still 1261 samples (~16%): almost
entirely `count_frequencies`' single-array histogram — a
load-increment-store dependency chain on one cache line — plus 83
samples of stable `sort_by` in the two-queue ordering (where the
comparator is already total, so stability buys nothing).

## Change

`count_frequencies` uses four interleaved counter arrays
(chunks_exact(4), one array per lane) with exact u32 sums combined in
fixed index order — integer addition is exact, so the counts are
identical to the single-array walk (byte-identical output by
construction). The two-queue sort switched to `sort_unstable_by`
(total-order comparator).

## Verification

- 33/33 corpus cells byte-identical to v0.21.85 (sizes matched
  exactly); round-trips green.
- Interleaved RUNS=100 user-CPU A/B vs v0.21.85 binary (box load
  12–31, direction consistent in all 4 rounds): 7.42→4.43, 3.36→2.85,
  6.93→2.91, 4.06→2.75 — **−15…−18%** on fits L1.
- Quiet-basis projection: fits zstd L1 T 3.0 → ~2.4 (ratio 0.82 on
  the v23 quiet measurements).
- Gates: 191+1 tests, fmt, clippy (CI invocation).

## Residual

Post-.85/.86 the fits L1 composition is: fast4 parse ~10%, histogram
(now ~6%), huffman stream ~4%, sequences negligible (fits L1 is
literal-dominated), remainder = diffuse emission assembly. No single
component >20% remains on the fast tier — the next movement needs the
per-op constants (SIMD histogram via std::simd, batched word writes)
or content-level levers (task 21's splitter chain).
