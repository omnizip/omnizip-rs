# 09 — zstd L1 text cells: emission measurement dominated the tier

- **Priority:** P1 (top open cluster on the v9 board)
- **Score evidence (v9):** words zstd L1 I=5.7, rustsrc 5.6, csv2m
  4.8 — with the parse primitives already cheap (v0.21.73).
- **Status:** done 2026-09-08 (v0.21.74)

## Root cause (profile-confirmed)

Sampling words L1 (3360 samples): the fast4 parse was 25%, huffman
literals ~17%, and **encode_sequences_bitstream 49% — of which 1535
of 1645 samples sat under `section_size_bits`**: the FSE table-mode
contest measured every candidate by running the FULL
byte-materializing bitstream (per block: predefined vs custom per
table, each = ctable builds + payload alloc + full encode), then
emitted the winner once. 93% of the bitstream work was measurement.

## Fix (v0.21.74)

`CState::encode_bit_count`/`flush_bit_count` share the exact
`encode_step` state arithmetic with the emitting path (a single
private core — the twins cannot diverge). `section_size_bits` sums
the counts with the writer's close() padding (`ceil((bits + 1) / 8)`)
— the identical size the emitting path produces, so every table-mode
decision is unchanged and output is byte-identical (verified across
the corpus).

Same-load stash A/B: words L1 0.064s -> 0.038s (-41%), fits L6
0.262s -> 0.204s (-22%). (Absolute user-time drifts ~25% with
shared-box load; only same-load A/Bs are valid comparisons —
documented in the board headers since v7.)

## Remaining (next levers, in cost order)

- words/rustsrc L1 residual T~4: huffman literals path (~17% — the
  repeat-table mode and weight-building measurement may have the same
  measure-by-materializing shape).
- zopfli_hq DP constant (sqlite q11 5.5, fits q11 4.7, rustsrc q11
  5.1 — task 04's remaining half).
- noto q9 5.2 / plists q5 5.2: small-file greedy+bank cells.
