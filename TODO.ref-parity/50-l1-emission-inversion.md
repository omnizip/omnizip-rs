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

## Session 2 measurements (2026-09-16)

### Fixed-size CTable arrays (REVERTED — net negative)

`state_table: Vec<u16>` → `Box<[u16; 512]>`, `symbol_tt: Vec<SCT>` →
`Box<[SCT; 256]>`: eliminates the symbol bounds check and gives static
bounds info for the state table. Byte-identical (55/55 at 11 levels).
Timing MIXED: dbdump L19 −4%, csv2m L19 −0.5%, words L1 **+6%** — the
always-512 u16 table wastes cache vs the exact-sized Vec (table_log=5
needs 64B; we allocated 1KB). The bounds-check saving doesn't
compensate for the cache-line spread. REVERTED.

### What's been ruled out on the writer

| optimization | result |
|---|---|
| Self-draining add_bits (remove eager flushes) | wash-to-regression |
| Fixed-size CTable (eliminate bounds checks) | words +6%, dbdump −4% — cache cost > check saving |
| Pre-allocating output Vec | already done (with_capacity) |

### The remaining hypothesis

The per-sequence ~10ns gap (ours ~22ns, C ~10-15ns) across ~700K
sequences is likely the accumulated cost of:
- 6 bounds-checked table indices per sequence (2 per state × 3
  states) where the C uses raw pointers
- The `i64 → usize` cast on the state_table index (adds a sign check)
- The per-`add_bits` mask computation (branch on nb_bits ≥ 32)

None of these individually dominates — each is 1-3 cycles — but
6 × 1-3 + 6 × 1-2 (add_bits) + 5 × 2 (flush) ≈ 25-50 extra cycles per
sequence × 700K = 5-12ms, which IS the ~50% residual on words L1.

### The path forward (next session)

The only remaining approach: **eliminate the bounds checks without
changing the memory layout**. The kernel-crate approach (task 49's
pilot) works technically — it was flat on the MATCHER because the
matcher's cost was elsewhere, but the EMISSION's cost IS the table
lookups. A targeted `read_u16_unchecked` on the exact-sized Vec
(keeping cache behavior identical) on state_table + symbol_tt is
the next move. Combined with a fused `encode_step_and_add_bits`
that avoids the intermediate tuple, this could halve the per-sequence
cost. The user must authorize the kernel crate's return.
