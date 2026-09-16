# Task 50 — the L1 inversion: the emission owns words-zstd-L1, and the mode search is the suspect

Status: open (2026-09-16; reopened by the owner's correct rejection
of the "language floor" — the 1.1x criterion stands)

## What the owner's challenge exposed

"Rust must be at worst 1.1x of C in all workloads — this is either an
algo problem or a code optimization problem." Three experiments:

1. Half-split at Fast on words L1: ~2% — minor, buys the fits size win.
2. Redundant guard trim in the fast matcher (5 branches/position):
   byte-identical, FLAT. Branches were predicted.
3. Fresh profile: **encode_section owns ~65%** of words zstd L1. The
   mode search walks each sequence stream 2-3x (per candidate mode)
   with a full build_ctable each — the C's ZSTD_selectEncodingType
   uses entropy heuristics and re-encodes nothing.

## Measured (2026-09-16, loop canon, load 30-40)

- **Mode-search walks = 10-16% of encode** on text L1 (words 3.22→2.89
  with walks disabled via the ZSTD_NO_WALK probe, csv2m 1.73→1.46).
  Real but NOT the 65% the profile suggested.
- **Self-draining BitCStream.add_bits** (removes the 5-per-sequence
  eager flush() calls): byte-identical (121/121 across 11 levels),
  but timing is a wash-to-regression (dbdump L19 consistently +5%,
  csv2m L19 mixed, words L1 flat). REVERTED — the removed flushes
  were already cheap; the drain-in-branch defeats optimization.

## The real 65%: the FINAL writer's per-sequence cost

After subtracting the 10-16% mode-search walks, ~50% of words L1
sits in the FINAL bitstream writer loop (encode_section +14444
offset region). This is ONE walk over ~700K sequences doing: 3 FSE
state-machine steps + ~6 add_bits + ~5 flush() calls per sequence.
The C's equivalent (ZSTD_encodeSequences_body) does the same 3 state
steps + ~6 BIT_addBits but with inline raw-pointer stores. The
per-sequence cost difference (ours ~22ns, C ~10-15ns) at this volume
= the dominant residual. Next session: attribute the writer's
per-sequence cost further (is it the FSE state machine's table
lookups through the CState struct, the BitCStream's Vec-target
stores, or the extras path's double indexing), then optimize.

## Plan (next session)

1. Instrument the writer loop: separate costs of state-machine steps
   vs bit writes vs extras indexing (env-gated counters).
2. Optimize the dominant component (likely the Vec-target bit writes:
   pre-allocate exact capacity, write via raw slices not extend).
3. Re-audit all 21+ "floor" cells under the 1.1x criterion with
   instruction-attributed profiles.
