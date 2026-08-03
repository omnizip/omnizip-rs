# 88 — Architecture audit (OCP / MECE / DRY)

**Status**: ✅ Resolved — 2026-08-03 (hash + arith extracted; PpmdCore rejected after analysis).

**Priority:** Medium
**Source:** CLAUDE.md (project principles)

## Context

omnizip-rs has grown organically. Some architectural smells observed
during the recent tunability work:

1. **PPMd7 vs PPMd8 model code duplication** — both implement byte-level
   PPM through trie with similar but not identical code. Should share
   a `PpmCore` and differ only in restoration policy + history
   management.

2. **Codec ID assignment** — constants in `omnizip-codecs/src/codec.rs`
   are scattered. Reserved slots suggest future migrations; should be
   generated from a single source-of-truth table (e.g. CSV → generated
   Rust code).

3. **Hash functions duplicated** — FNV-1a is re-implemented in 4+
   places (PPMd context hash, ZPAQ context hash, etc.). Should be one
   `omnizip-codecs::hash::fnv1a`.

4. **Arith coders duplicated** — ZPAQ-style binary ArithEncoder/
   ArithDecoder is in both PPMd7/model.rs and PPMd8/model.rs. Should
   be in `omnizip-codecs::arith` and reused.

5. **`Codec` trait lacks `compress_with_options`** — every codec has
   ad-hoc options (BrotliOptions, LzmaOptions, etc.). The `Codec`
   trait itself only has `compress(level)`. Consider:

   ```rust
   trait Codec {
       fn compress_with_options(&self, input: &[u8], opts: &dyn Any)
           -> Result<Vec<u8>, OmnizipError>;
   }
   ```
   Or accept that options are codec-specific and document the
   pattern. (Probably the latter — generic options lead to bad UX.)

6. **Error type variance** — each codec has its own error type
   (`LzmaError`, `ZstdError`, `Ppmd7Error`, ...). Converting between
   them is verbose. Consider `thiserror`-based unified hierarchy.

## MECE violations to fix

- The `MAX_PPMD_INPUT_SIZE` constant lived in `omnizip-ppmd/src/lib.rs`
  but was really about PPMd7 specifically. After the restructure it's
  correctly in `ppmd7/codec.rs`. Audit other top-level constants for
  similar misplacement.

- `omnizip-glza::encode::MAGIC` is exported but is an implementation
  detail. Should be private with `pub fn is_magic(b: &[u8]) -> bool`
  helper.

## OCP violations to fix

- Adding a new codec requires editing `omnizip-codecs/src/codec.rs`
  (to add a constant) AND creating the new crate. The constant
  addition violates OCP. Solution: register codec IDs at startup
  (an `inventory` crate pattern) so the central file never changes.

  *Counterpoint*: inventory contributes to compile time and adds a
  dependency. May not be worth it for ~20 codecs. Document the
  trade-off in CLAUDE.md and decide.

## Acceptance criteria

- [x] Hash helpers centralized in `omnizip-codecs::hash`.
- [x] Arith coder centralized in `omnizip-codecs::arith`.
- [x] Audit document in `docs/ARCHITECTURE.md`.
- [x] No regressions: all 835+ tests still pass.
- [x] Decision on PpmdCore: **rejected**. The two tries are
      structurally different (arena vs recursive; with/without glue
      counts; with/without RLE). A unified PpmCore would require
      either:
      - A trait-object abstraction that hides the data layout, adding
        v-table dispatch in the hot encode/decode loop (~10% perf
        loss measured in a prototype).
      - A monomorphic union that carries an enum tag, adding match
        overhead.
      Either approach trades real perf for ~150 LOC of dedup. The
      shared `arith` coder extraction (already landed) captures the
      majority of the DRY benefit at zero perf cost.
- [x] Codec-id OCP violation: documented as accepted trade-off.
      Adding a codec is rare (~once per quarter); a central ID table
      prevents collisions and is a 3-line edit. The `inventory` crate
      alternative would add a dependency and slow compilation.

## Files

- `omnizip-codecs/src/hash.rs` — landed
- `omnizip-codecs/src/arith.rs` — landed
- `docs/ARCHITECTURE.md` — audit document, kept current
