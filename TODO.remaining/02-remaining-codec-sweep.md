# Task 02: Remaining codec broad-corpus sweep

## Status: done (2026-08-29) — bzip2 fixed; deflate/lz4 residuals filed

## Sweep results (ours / reference CLI, same level)

**bzip2** (before → after this round): every cell was 1.05–1.23x.
Root causes (both fixed, shipped 0.21.20):
1. `Bzip2Codec` emitted a Ruby-parity custom container (single Huffman
   table, byte-aligned headers) — rewired to the standard `.bz2` writer
   in `bz2/mod.rs` (the Ruby format remains available in-module).
2. The standard writer used ONE Huffman table (all selectors = 0);
   ported upstream's `sendMTFValues`: 2–6 tables, per-50-symbol
   selector optimization, 4 refinement passes, MTF-coded selectors,
   delta-coded lengths, 17-bit table cap.
3. Block splitting budgeted INPUT bytes; the wire format caps a
   block's RLE1 OUTPUT at `100000*level − 19` (`nblockMAX`).
   Low-redundancy data overflowed it and `bzip2 -d` rejected the
   stream (data-integrity error). New `block_chunks()` splits on the
   RLE1 budget, runs straddling the budget split across blocks.
4. Empty stream was missing its trailing CRC (14-byte empty member
   now, as the CLI emits).

After: all 10 corpora × {1,9} within **0.999–1.001x** of `bzip2`.
Regression gate: `interop_with_system_bzip2` (round_trip.rs) pipes
our output through `bzip2 -d`, incl. multi-block periodic data.

**deflate** (vs python zlib, same level): lv1 0.821–1.018x (beats
zlib-1 nearly everywhere), lv9 0.968–1.071x. Root cause of the lv9
gap: `omnizip-libdeflate::compress` ignores the level entirely
(`let _ = level;`) — one dynamic-vs-fixed-vs-stored contest, no
tiered lazy matching. → follow-up task 11.

**lz4** (lz4_flex): fast tier 0.92–**1.227x** (arial 1.227x, rfc
1.098x — poor on low-redundancy data); HC tier 1.000–1.042x.
→ follow-up task 12 (fast tier).

**snappy**: snzip unavailable on this box — skipped per task.

## Acceptance

- Full matrix recorded above and in memory.
- Both >1.2x cells root-caused: bzip2 (fixed), lz4-fast on fonts
  (filed as task 12).
