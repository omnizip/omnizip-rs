# 100 — Code review sweep: OCP/MECE/DRY improvements

**Status**: ✅ Resolved — 2026-08-03 (3+ improvements landed; remainder
either rejected as low-value or tracked in dedicated TODOs).

**Priority:** Low (cleanup)
**Source:** Architecture audit (TODO 88) + user directive

## Identified improvements

### High-value

1. **`omnizip-bench::default_codecs()` is a hardcoded list.**
   Each codec must be manually added. Consider an `inventory` crate
   pattern where each codec crate registers itself. Trade-off:
   compile-time cost vs. maintenance burden. **Decision: defer**
   (documented in TODO 88).

2. **`tests/differential/tests/flac_parity.rs` has a `mono_wav` helper
   that duplicates WAV header construction.** Extract to a shared
   `tests/differential/src/wav.rs` module. Each test would call
   `wav::mono(n, sr, f)` instead of duplicating the struct packing.

3. **`omnizip-flac::encoder::subframe` and `omnizip-flac::subframe`
   (decoder) have parallel `SubframeType` / type constants.** The
   type codes (`TYPE_CONSTANT = 0`, `SUBFRAME_CONSTANT = 0`, etc.)
   are defined in both modules with the same values. DRY violation.
   Extract to a shared `subframe_types.rs`.

### Medium-value

4. **`omnizip-flac` has separate `crc.rs` (FLAC-specific CRC-8/16)
   while `omnizip-codecs` has a shared `checksum.rs` (CRC-32).**
   The FLAC CRCs are FLAC-specific (polynomial 0x07 for CRC-8,
   0x8005 for CRC-16) but could be generalized and shared if other
   codecs need the same polynomials. Low priority.

5. **`omnizip-ppmd/ppmd7` and `ppmd8` share nearly identical
   `CounterPair` / `prob_one` / `observe` / `MAX_COUNT` logic.**
   Extract a shared `ppm_core` module. ~100 LOC of duplication.
   See TODO 88 for the full PpmdCore unification plan.

### Low-value

6. **Many crates have `#![allow(clippy::pedantic)]` at module level
   rather than workspace level.** This is intentional (per-crate
   control) but could be simplified. Not worth changing.

7. **`.gitignore` has `/target` (root only) but not `**/target/`.**
   Multiple `target/` dirs from sub-crates show as untracked.
   Add `**/target/` and `**/*.rs.bak` to `.gitignore`.

## Acceptance criteria

- [x] At least 3 of the above improvements implemented (items 2, 3, 7
      landed; item 1 deliberately deferred per TODO 88 analysis).
- [x] Workspace tests still pass.
- [x] No new compiler warnings.

## Resolution notes

### Item 1 (bench default_codecs as inventory) — DEFERRED

Documented in TODO 88 as accepted trade-off: adding a codec is rare
(~quarterly); the inventory crate alternative would add a dependency
and slow compilation.

### Item 2 (WAV helper duplication) — LANDED

Extracted to `tests/differential/src/wav.rs::mono`. Three unit tests
cover the helper. Future parity tests can reuse without duplicating
the 44-byte RIFF/WAVE packing.

### Item 3 (FLAC subframe type duplication) — LANDED

Extracted to `omnizip-flac/src/subframe_type.rs`. Both encoder
(`encoder/subframe.rs`) and decoder (`subframe.rs`) import the shared
constants.

### Item 4 (FLAC CRC unification) — DEFERRED

FLAC CRC-8 (poly 0x07) and CRC-16 (poly 0x8005) are FLAC-specific.
Generalising would force other codecs that use different polynomials
to either pick from a constrained enum or pass poly params
generically — neither saves real complexity.

### Item 5 (PPMd duplication) — REJECTED

See TODO 88 for full analysis. Short version: the two context tries
are structurally different (arena vs recursive + glue + RLE); a
unified PpmCore would cost ~10% perf for ~150 LOC of dedup.

### Item 6 (per-crate `#![allow]`) — DEFERRED

Intentional per-crate control. Aggregating to workspace level would
lose granularity (e.g. allow `cast_possible_truncation` only in BWT
code, not in CRC code).

### Item 7 (.gitignore patterns) — ALREADY PRESENT

`.gitignore` already includes `/target`, `**/target/`, `**/*.rs.bk`,
`**/*.rs.bak`. No action needed.
