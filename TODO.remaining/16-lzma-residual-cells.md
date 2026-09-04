# 16 — LZMA residual cells: re-measure + close

- **Priority:** MEDIUM
- **Depends on:** nothing
- **Estimated effort:** 1 day
- **Status:** pending

## Current state

Sizes are within bar everywhere (≤1.022x vs `xz -6`). Times were last
cleanly measured before the shared-box load made readings unreliable:

- fits4m 1.08–1.10x, mix2m 1.05–1.19x, m329_a 1.12x, big100m 1.14x
  wall (0.58x best-of-5 after the u128 match_len pass), m329_c last
  clean reading 1.34x (suspect — load-inflated).

The user bar is ≤1.2x ref time. The suspects are m329_c and mix2m.

## Plan

1. Quiet-machine re-measure (best-of-5 interleaved ours-vs-ref, user
   CPU time, `xz -6 -c` as the oracle) across fits4m / mix2m /
   m329_a/b/c / big100m at -6 and -9.
2. If m329_c or mix2m still >1.2x: profile (`sample` +
   `-C force-frame-pointers`), apply the established playbook —
   primitives audit first (the q1 lesson: per-byte loads where C does
   word ops), then algorithmic.
3. Corpus note: sweep-corpus `fits4m.bin` is the REGENERATED synthetic
   (not the original that measured ~1.006x); source a real FITS file
   under task 15 before treating fits4m cells as signal.

## Acceptance

- Fresh table recorded in this file; every cell ≤1.2x time or
  root-caused with a documented trade (like the q2 5.3x that BUYS
  −25% size).
- No size regressions (regression gate + interop oracle
  `xz -d -c` byte-identical).
