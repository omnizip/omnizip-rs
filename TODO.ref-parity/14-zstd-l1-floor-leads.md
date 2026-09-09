# 14 — zstd L1 floor: the two remaining leads, inspected and deprioritized

- **Priority:** P3 (cells at I 3.0-4.4, post-v16)
- **Status:** inspected 2026-09-10; both leads DEPRIORITIZED with
  evidence. Reopen either only if the corpus or tier contract
  changes.

## Composition (words zstd L1, 1,972 samples, post-v0.21.78)

fast4 parse 41% (812), sequences emission 43% (encode_section 850 +
size_bits 218 — the counting path already collapsed from 1,535),
literals ~19% (221 + huffman_stream 164), checksum 68.

## Lead 1: encode_sequences_bitstream's per-block Vec + copy

Each block emission allocates `Vec::with_capacity(dst.len())`, fills
it via the growable BitCStream, then `copy_from_slice`s into dst.
Removing it requires a slice-sink BitCStream — but the current size
estimate (80 bits/seq + padding) is NOT an upper bound: worst case
per sequence is extras 63 (16+16+31) + FSE state bits 26 (LL9+OF8+
ML9) = 89+ > 80. A safe direct write needs the bound raised to
~117 bits/seq (over-allocating the payload buffer) or overflow
fallback plumbing. Expected gain: the alloc+copy is maybe 10-15% of
the 850-section samples = ~5% of words L1 (I 3.6). Not worth the
neutral-risk rewrite cycle (see tasks 12/13's pattern).

## Lead 2: the literals measurement path (task 09's old note)

encode_literals_internal at ~19% share. Unchecked for the
measure-by-materializing shape that paid 41% on the sequences side —
but the remaining share is under the level where that class of fix
moved cells (the sequences fix removed 93% of bitstream work from a
49% share). At 19%, even a halving moves I by ~0.1 on cells at 3.6.
Deprioritized.

## Floor statement

v16 (single-window, user-CPU): 77 cells, zero whack, median I=1.44,
one cell >4.5 (fits brotli q11 — task 12's accept). The zstd L1
cells sit at 3.0-4.4 = the fast4 parse constant plus irreducible
emission; the brotli residuals are accepted or by-design. Further
movement needs either the instrumented H10 node-count study (task
12's reopen condition) or a corpus/tier-contract change.
