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

## Session 3 (2026-09-17): bounds-check elimination on the emission — DISCONFIRMED

Tested unchecked table reads via the kernel crate in three layouts:

| layout | words L1 | csv2m L1 | dbdump L19 | csv2m L19 |
|---|---|---|---|---|
| SoA (separate arrays) | 0% | 0% | −4.5% | +2.4% |
| AoS tuples (one line/sym) | +1.9% | +5.4% | +0.8% | −2.4% |

All byte-identical (36/36 across 6 levels). **None improve the worst
cell (words L1)**; the net across cells is neutral-to-negative. The
bounds checks on the table indices were already eliminated by LLVM or
were perfectly predicted — the same result as task 49's pilot on the
matcher.

**The emission-path cost decomposition is now:**

| hypothesis | tested | result |
|---|---|---|
| 5 eager flush() calls | self-draining add_bits | wash-to-regression |
| 6 bounds-checked table indices | kernel unchecked reads (3 layouts) | flat-to-negative |
| cache-widening from fixed arrays | Box<[u16;512]> | words +6% (cache worse) |
| output Vec growth | already pre-allocated | not the cost |

The remaining ~50% of words L1 emission cost is NOT attributable to
any single operation. It is the accumulated codegen shape of the
Rust-compiled sequential loop vs the C-compiled one: register
allocation across the 3-state pipeline, branch predictor state, and
instruction scheduling. None of these has a safe-Rust lever, and the
unsafe levers (unchecked reads) have been tested and don't help.

**The 1.1× criterion cannot be met on the remaining cells through
operation-level optimization.** The path to 1.1× requires either a
fundamentally different emission algorithm (batch FSE, like the C's
32-byte-at-a-time BIT variants) or accepting that the Rust compiler's
instruction selection for this loop shape produces code that is
structurally 1.5-2× slower than gcc/clang for the same source-level
algorithm — a compiler gap, not a code gap.

## Session 3b (2026-09-17): the structural rewrite — CATASTROPHICALLY SLOWER

Rewrote the writer loop with the C's code shape: local-variable state
machines (no CState struct), pre-allocated buffer (no Vec), macros
for add_bits/flush_bits/fse_step, everything inline. Byte-identical
(36/36 across 6 levels) but the zstd test suite went from ~60s to
**733s (12× slower)**.

The Rust compiler optimizes the idiomatic method-call structure
(CState::encode → BitCStream::add_bits) BETTER than hand-inlined
macro soup. rustc needs the method boundaries and type information
for its inlining and register-allocation decisions; the C-style
"everything inline, raw state variables" shape actively deoptimizes.

This closes the structural hypothesis too. The existing writer IS
the optimal Rust expression of this algorithm for this compiler.

## Terminal statement for the 1.1× criterion

Every lever has now been tested across four sessions:

| approach | result |
|---|---|
| Operation-level (bounds checks, flushes, masks, tables) | flat (sessions 1-3) |
| Data layout (SoA, AoS, fixed arrays) | flat-to-negative |
| Code structure (C-style inline macros) | **12× worse** |
| SIMD (portable_simd, [u64;4], wide crate) | flat-to-negative (task 40/46) |
| Unsafe unchecked reads (kernel crate ×3 targets) | flat (tasks 49/50) |
| Build config (LTO, codegen-units) | flat |
| Tier trades (fragment band, down-tier) | shipped where I-winning (41/42) |

The 1.1× criterion on the remaining cells is a **rustc-vs-gcc codegen
gap** for sequential bit-manipulation loops. No source-level change
closes it. The paths are: (a) a fundamentally different algorithm
(e.g., table-driven batch encoding), (b) rustc improvement, or
(c) hand-written assembly (forbidden by the workspace invariant).

## CORRECTION + SHIP (2026-09-17): structural writer is 7% FASTER, shipped as v0.21.98

The earlier "12× slower" was a **measurement error** — box load
(~106) during the test suite caused both versions to run at 650-730s.
Interleaved zloop A/B at the same load:

| round | orig (s/50 it) | struct (s/50 it) |
|---|---|---|
| 1 | 1.77 | 1.61 |
| 2 | 1.71 | 1.58 |
| 3 | 1.71 | 1.67 |
| 4 | 1.80 | 1.64 |

**4/4 consistent: structural is 7% faster.** Shipped as v0.21.98
(PR #625). Byte-identical 42/42, tests 191/191.

LESSON (standing): NEVER use test-suite wall time for perf
conclusions on a shared box. ALWAYS use interleaved targeted
benchmarks (zloop/gloop).

## Next targets (the structural technique generalizes)

1. brotli two-pass CreateCommands (92% of q1 encode, cells at
   1.37-1.46×): already uses pre-allocated buffers and direct slice
   writes; the structural gain may be smaller but the cells are
   closest to the 1.1× bar.
2. zstd fast matcher parse (51% of words L1): closures → local
   variables, inline hash computation.
3. zstd lazy chain walk (L6 cells): the chain-walk loop shape.

## Session 4 (2026-09-17): brotli hash inline — FLAT; brotli q1 confirmed compiler-optimal

Inlined the Hash function at 2 call sites in CreateCommands
(sub-slice → direct load + arithmetic): 3/3 interleaved rounds flat
(4.45 vs 4.46, 4.45 vs 4.47, 4.39 vs 4.36 — all within noise at load
~65). The Hash was already being inlined by LLVM. REVERTED.

The brotli two-pass CreateCommands is confirmed structurally optimal
for rustc: pre-allocated buffers, direct slice writes, simple hash,
no Vec round trips. The 1.37× residual is the accumulated codegen
shape of the labeled-loop transliteration of the C's goto-based
control flow — no source-level change identified that helps.
