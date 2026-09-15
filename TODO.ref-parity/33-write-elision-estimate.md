# Task 33 — write-elision for estimate_partition (designed, load-blocked)

Status: closed (2026-09-15; slice 1 shipped v0.21.95; slice 2 measured out)

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


## Slice 1 (shipped v0.21.95): scratch reuse across trials

`estimate_partition` allocated a fresh `Vec::new()` per trial (3+ per
derive_splits node) whose ONLY consumer is `.len()` — one buffer now
threads through the recursion, cleared per trial, capacity retained
(1<<17 pre-allocated). Byte-identical by construction; 33/33 corpus
cells L1/L6/L19 identical to v0.21.94. Quiet-load-9 A/B: words L19
1.09→1.02s (−6%), csv2m 1.18→1.13 (−4%), small files flat — the trial
cost is the emission WORK, not the allocations, so the full no-write
variant (the design above) remains the lever. words L19 board I 1.95
→ ~1.85.

Fresh profile confirming the design's target (words L1, 5s sample):
encode_frame_into (parse) 51%, encode_content_parts→encode_section
29% — the L1 cells' residual is per-op floor; the L19 derive-splits
share is where write-elision applies.


## Slice 2 plan (next session — execute mechanically)

`encode_content_parts` (block.rs:1277) materializes EVERY candidate
before length-comparing: `raw_literals` (raw write), `huf_literals`
(full Huffman payload), `treeless_literals` (second full payload), and
the sequences section per mode. A trial needs only the winning LENGTH.

1. `huffman::encoder`: add `measure_encode_literals(literals,
   treeless) -> Option<usize>` = header bytes + `Σ f·len` bits via the
   proven `huffman_cost_from_hist` (Σ over the two-queue lengths),
   rounding the payload UP to bytes; header size = write the 1-byte
   header + reuse the ncount scratch (O(256)) for `write_ncount`'s
   length. Same err contract as the materializing variant.
2. `estimate_partition`: call a new `measure_content_parts` that uses
   (1) for the three literal candidates and the `.87` `stream_payload`
   machinery for the sequences section (real pick_table decisions,
   real rep-state carry — the gate-2 lesson: NO histogram-level
   approximation survives csv2m's mode search).
3. `derive_splits`/the byte-based final `write_split_blocks` stay
   exact; only trial scoring switches to bits.
4. Acceptance (unchanged): csv2m L19 within +0.2% of v0.21.89 bytes
   (152,843), all L1/L6/L16-19 corpus round-trips, quiet-box T per
   canon; expected derive −30–50% (writes are 40–60% of trial cost —
   slice 1 proved allocations are NOT the cost, the writes are).


## Closure (2026-09-15): slice 2 is not the L19 lever

Fresh loop-harness profiles (6s samples, zloop_tmp):
- dbdump L19: `encode_frame_into` = **97%** of encode (the btopt DP);
  `compress_block_opt_with_prefix` ≈ 65%, `insert_bt_and_get_all_matches`
  ≈ 30% (of which `count_abs` ≈ 25% — already u64-word-stepped with
  XOR/trailing_zeros, i.e. at the scalar floor). derive_splits +
  estimate_partition = **0.7%**.
- csv2m L19: derive ≈ 15% (heavier splitting) — a perfect write-elision
  (−50% of derive) caps at −7% total. csv2m L19 I 2.4 → ~2.25 at best.

The design's "L16-19 column −8-12%" estimate came from the task-21
world (L5-L12-gated splitter trials); the default column's splitter is
already cheap (task 25's caching + slice 1) and the btopt DP is the
whole cost. Same closure class as task 27: measured, documented, not
retried. The slice-2 plan below is left for reference only.

**The zstd default columns are parse-floor:** L1 (words 3.65 = 51%
parse + 29% seq emission) and L19 (dbdump 2.33 = 97% opt DP) both sit
at the task-30 safe-Rust per-op floor; the escape hatches remain task
36's three (portable_simd, task 21's 5× emitter cut, size-for-time).
