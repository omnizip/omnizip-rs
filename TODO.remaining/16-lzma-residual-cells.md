# 16 — LZMA residual cells: re-measure + close

- **Priority:** MEDIUM
- **Depends on:** nothing
- **Estimated effort:** 1 day
- **Status:** done 2026-09-04

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

## Results (2026-09-04, shared box) — task CLOSED, within bar

The original fixtures (mix2m, m329_a/b/c, big100m) died with /tmp and
have no in-tree generator; the durable `~/sweep-corpus` is the
reproducible replacement. Fresh `xz -6` table:

| file | size ours/ref | time ours/ref |
|---|---|---|
| fits4m.bin | 996/1008 (0.988 beat) | 0.088/0.053s (1.65x — sub-100 ms cell, startup scale) |
| csv21m.bin (17.8 MB) | 631,620/747,260 (**0.845 beat**) | 13.5/11.2s (1.20x, at bar) |
| words.txt | 1.0004 | 0.95x |
| rustsrc.txt | 1.0000 tie | 1.05x |
| dbdump.txt | 1.0035 | 1.15x |
| arial.ttf | 1.0002 | 1.10x |
| bin2 | 1.0012 | 1.19x |
| rand.bin | 1.0137 (incompressible overhead) | **0.88x (faster)** |
| csv2m.bin | 1.0017 | 1.12x |

Every size ≤1.014x (bar 1.2x); every time ≤1.2x except the 88 ms fits
cell. The old m329_c/mix2m suspects are unreproducible; nothing on the
durable corpus needs action.

## Acceptance

- Fresh table recorded in this file; every cell ≤1.2x time or
  root-caused with a documented trade (like the q2 5.3x that BUYS
  −25% size).
- No size regressions (regression gate + interop oracle
  `xz -d -c` byte-identical).
