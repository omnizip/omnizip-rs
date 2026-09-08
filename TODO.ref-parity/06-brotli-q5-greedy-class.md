# 06 — brotli q5 class: greedy tier ships 6-25% larger than ref at 7-18x time

- **Priority:** P0 (contains the only S>1 whack cell)
- **Score evidence (v5):** plists q5 **I=10.9 (T=8.8x, S=1.246)** —
  the worst class on the board (paying time AND shipping larger);
  fits q5 I=16.8 (T=17.9x, S=0.939); install q5 I=9.0 (S=1.064);
  icons q5 I=7.6 (S=1.111).
- **Status:** done 2026-09-08 (v0.21.69)

## Root cause (confirmed — not the greedy tier itself)

The lazy lookahead hypothesis was wrong: the greedy tier already
runs lazy+lazy2 (brotli_quality_config q4-9 text rows). The real
gap: the BankMatchFinder (the reference H5/H6/H9 port WITH
short-code distance probes) was wired ONLY into the >=1 MiB chunked
path; the sub-1 MiB single-metablock path
(encode_huffman_chunk_into) created only the chain finder and passed
bank_mf: None. Sub-1 MiB structured text lost every repcode match —
exactly what JSON property lists live on (plists q5 S=1.246).

## Plan

Extracted the bank construction into `build_greedy_bank` (shared by
both paths) and wired it into the sub-1 MiB path.

### Measured (post-fix, load ~20-40)

plists q5: 160,057 -> 130,653 (S 1.246 -> **1.017**, T 9.9 -> 5.2,
I 12.3 -> 5.3); plists q9 S 1.257 -> 1.071 (I 5.3 -> 1.9);
icons q5 S 1.111 -> 1.003; install q5 S 1.064 -> 1.008;
rfc q5 S 1.064 -> 1.025; sqlite q5 S 1.128 -> **0.994 (beats ref)**;
noto q5 S -> 1.012; dbdump q5 S -> 1.000. Times halved on every
cell. >=1 MiB outputs byte-identical (same helper, same params).

## Acceptance

- q5 cells: S <= 1.05 and I <= 4 on plists/fits/install/icons.
- q2-4 unchanged (still greedy) unless lazy wins there too.
