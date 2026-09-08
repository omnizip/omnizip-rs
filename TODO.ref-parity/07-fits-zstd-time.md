# 07 — fits zstd time class: L1 I=12.8, L6 I=9.0 (v7 board)

- **Priority:** P0 (the two zstd whack cells on the v7 honest board)
- **Status:** DONE 2026-09-08 (v0.21.72 sub-split gating + v0.21.73
  window-form primitives). fits L6 I 7.8 -> 4.2; the zstd L6 column
  now sits at I 1.0-4.2 across the corpus.

## Sub-split gating (v0.21.72)

The 16 KiB sub-block split (heterogeneous chunks whose halves'
distributions diverge) ran at every level; the reference block-splits
only at the opt tiers. On fits4m it is a size-over-time mis-tier:

- L1: the split costs 71% of encode time for a 17% size win
  (0.040s -> 0.140s user, 3.78MB -> 3.15MB; I 4.6 -> 13.6).
- L6: 26% time for 9% size (0.368s -> 0.500s; I 7.8 -> 9.7).

Gated to Btopt/Btultra/Btultra2 (`ZSTD_SUBSPLIT_ALL` restores it
everywhere for measurement). Measured: **fits L1 I 12.8 -> 4.4**
(T 4.2x, S 1.052), **fits L6 9.0 -> 7.8**. All other zstd cells
byte-identical (homogeneous files never split; the synthetic
regression rows are stationary — gate green without a baseline
change).

## Remaining: the lazy parser constant on binary

fits L6 without the split: 0.364s user vs ref 0.049s (T=7.4x) for
S=1.044. The per-position loop is a faithful port but each op pays
Rust bounds checks the C doesn't: `read32`/`read64` via
`from_le_bytes` byte arrays, the `count` byte-tail indexing, and
per-slot reject loads. Fix shape: slice-window `count` (hoist one
bounds check per side, as the bank finder's `match_len_scan` did —
measured 5-8x there), `ptr::read_unaligned`-free fused loads via
`chunks_exact`, and precomputed reject bytes. Acceptance: fits L6
T <= 4x with byte-identical output (pure port-speed work).

## Window-form primitives (v0.21.73)

The lazy parser's per-op constant was byte-indexed loads: read32/
read64 from arrays of individually bounds-checked byte indexes (up
to 8 panic checks per load), count stepping with two checked loads
per tail byte, and every compare re-checking both operands. Both
match windows are now sliced out once and stepped as equal-length
chunks_exact iterators (the shape the bank finder's match_len_scan
proved at 5-8x); compares are slice equality; hash4/count_match in
the shared match finder (fast/greedy/legacy paths) got the same
treatment. Byte-identical output (stash A/B per rewrite).

Measured (user CPU time): fits L6 0.364s -> 0.198s (I 7.8 -> 4.2),
words L6 4.7 -> 2.8, csv2m L6 4.4 -> 2.6, every small-file L6 cell
1.0-2.6. Remaining zstd residuals (new task 09): words/rustsrc L1
I~5.6 — the fast4 parse on text; profile before touching (the
count/hash primitives are now shared and cheap, so the cost is
elsewhere: per-position insert loop or per-block emission).
