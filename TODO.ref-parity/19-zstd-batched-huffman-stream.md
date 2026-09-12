# Task 19 — zstd: batched 4-symbol Huffman literal stream (L1 remainder)

Status: measured-out (2026-09-12, post-v0.21.83 re-profile)

## Update: premise invalidated

The post-.83 hot-loop profile of fits4m L1 (RUNS=200, 5317 samples) shows
`encode_huffman_stream` no longer registers as a hot frame (<2% — the
task-17 flush batching plus inlining absorbed it). The literal path is now
~95% `build_weights` (1468 of 1482 samples under
`encode_literals_internal`, 27.6% of encode): package-merge's per-level
`sort_unstable_by_key` over ~(256 leaves + ~255 packages) x 11 levels x
32 blocks (~800 samples incl. sort frames) plus the frequency histogram.

The known O(n) fix — merging the two sorted runs (leaves; pair-sums of a
sorted list are non-decreasing) instead of sorting the concatenation —
is NOT byte-identical in general: `sort_unstable_by_key` breaks weight
ties in an implementation-defined order, tied coins change which sets
survive `truncate(bound)`, and tied-frequency symbols can end up with
different lengths (different weights table on the wire). Matching the
unstable sort's exact tie permutation analytically is not feasible.
Recorded as the hazard that blocks this lever; do not "optimize" the
sort away without a tie-equivalence proof or a fresh corpus-wide identity
sweep that passes.

Original plan (not executed, premise gone):
Parent: task 17 (shipped v0.21.83), tasks 09/14 (prior L1 work)

## Evidence

Post-.83 profile share (task 17's fits4m L1 hot-loop sample, adjusted for
the shipped fixes): `encode_huffman_stream` remains ~15% of L1 encode —
the per-literal loop (table lookup + `add_bits` + occasional flush) over
4 streams per 128 KB block. The C reference's `HUF_compress1X_usingCTable`
processes the stream 4 symbols at a time with a preloaded bit-window
(state machine over `BITCStream` with explicit carry handling), halving
the per-symbol instruction count.

## Plan

Rewrite `encode_huffman_stream` (omnizip-zstd/src/huffman/encoder.rs) to
accumulate four symbols into the u64 container before a flush decision —
the container already holds up to 63 bits, and codes are ≤ 11 bits, so
four codes fit when the accumulator is drained at bit_pos > 20 instead of
52. The REVERSE-order emission must be preserved (decoder reads MSB-first
from the high end). Byte-identity is guaranteed by the same argument as
task 17's flush batching: `flush()` drains whole bytes and keeps the
remainder, so the emitted byte sequence is independent of when flushes
happen.

## Acceptance

- 33-cell corpus sweep byte-identical (11 files × L1/L6/L19).
- RUNS-loop A/B ≥5s base on fits/words L1; ship only if T improves.
- Full gates + release drill.

## Non-goals

fits L1 S=1.052 (task 14 inspected the size leads — Vec+copy ~5%,
literals share sub-threshold); csv2m L6 T re-measure (needs a quiet box).
