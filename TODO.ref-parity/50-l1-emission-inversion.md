# Task 50 — the L1 inversion: the emission owns words-zstd-L1, and the mode search is the suspect

Status: open (2026-09-16; reopened by the owner's correct rejection
of the "language floor" — the 1.1x criterion stands)

## What the owner's challenge exposed

"Rust must be at worst 1.1x of C in all workloads — this is either an
algo problem or a code optimization problem." Reopened accordingly;
three experiments this session:

1. Half-split at Fast on words L1: 3.39 -> 3.32s (~2%) — minor waste,
   but it buys the -5.44% fits size win; left alone.
2. Redundant guard trim in the fast matcher (5 branches/position
   removed — the guards re-derive loop invariants the C never checks):
   byte-identical, timing FLAT. The branches were predicted/fused.
3. Fresh symbolicated profile of words L1: **encode_section owns ~65%
   of the encode** (encode_frame_into splits ~1200 parse / ~1350 with
   1116 under encode_content_parts -> encode_section), NOT the
   matcher. The six-confirmation "parse floor" story was built on an
   under-read profile.

## The suspect (next session's lever)

`encode_section`'s mode search (task 25's cached-cost machinery) runs
`stream_payload` per candidate mode — predefined AND FSE for each of
the 3 streams — and each call walks the FULL code array
(`encode_bit_count` per symbol) plus a complete `build_ctable`.
Per block that is 6+ full walks + 6 table builds before the real
emission; the C's ZSTD_selectEncodingType picks modes from cheap
entropy heuristics and re-encodes nothing. On words L1 (~20 blocks,
~700K sequences) that multiplies the per-sequence work several-fold.

## Plan

1. Single-pass cost accumulation: one walk computing all candidate
   modes' costs together (the bit counts share the same symbol
   stream), or entropy-preselection like the C (pick the mode before
   any walk; verify against the exact chooser via the corpus sweep).
2. Re-profile; then the parse (~35%) gets the same treatment only if
   still needed for the 1.1x bar.
3. The 1.1x criterion replaces "floor" language in all remaining
   cells: brotli q1 (CreateCommands), zstd L6/L19 walks get the same
   emission-first audit.
