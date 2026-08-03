# 100 — Code review sweep: OCP/MECE/DRY improvements

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

- [ ] At least 3 of the above improvements implemented.
- [ ] Workspace tests still pass.
- [ ] No new compiler warnings.

## Files

- `tests/differential/src/lib.rs` — add WAV helper
- `omnizip-flac/src/encoder/subframe.rs` + `omnizip-flac/src/subframe.rs` — extract shared types
- `.gitignore` — add patterns
