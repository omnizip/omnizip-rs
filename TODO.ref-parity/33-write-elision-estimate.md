# Task 33 — write-elision for estimate_partition (designed, load-blocked)

Status: pending (2026-09-14; timing verification blocked — box load 142
from other users' CPU burners; the canon forbids conclusions >20)

## Design (ready to execute on a quiet box)

`estimate_partition` trials run full `encode_content_parts` per
candidate — including bitstream MATERIALIZATION (4 literal Huffman
streams + 3 FSE streams per trial, ~3 trials per recursion level).
The no-write variant keeps decisions bit-identical-or-swept:

1. Literal section: histogram + two-queue lengths (`Σ f·len`, the
   `huffman_cost_from_hist` helper already written and proven within
   6 bytes on words in the gate-2 work) + weights-wire SIZE via
   `write_ncount` into a reused scratch (O(256), cheap).
2. Sequence section: the .87 cached-cost machinery (`stream_payload`
   walks with all four mode candidates — fse/pre/rle/repeat — real
   `pick_table` decisions, real rep-state carry between partitions).
3. Derivation compares BITS; `write_split_blocks` and the byte-based
   shield stay exact. Near-tie flips (byte vs bit rounding) are
   possible — the corpus sweep is the arbiter (csv2m L19 must hold
   within +0.2% of 152,843).

Also test first: plain scratch pre-allocation
(`Vec::with_capacity(1<<17)`) — the growth-doubling of thousands of
trial buffers may be a large slice of the cost by itself.

## Expected

Writes ≈ 40–60% of trial cost → derive_splits −30–50% → the L16–19
default column −8–12% (csv2m L19 T ~2.4 → ~2.1–2.2; 5 cells move
~0.2 I). Does NOT clear task 21's L5–L12 bar (needs 5×; this is
~1.4–2×) — this is the default-column lever only.

## Acceptance

- csv2m/words/plists/noto/sqlite/dbdump L19 within ±0.2% of v0.21.89
  (sweep), round-trips green, quiet-box T per canon (load <10,
  ≥2s/side, /usr/bin/time user).
