# TODO 148: Code review sweep — OCP/MECE/DRY

## Problem

Code has grown organically. Time for a structured review pass to
catch:

- **OCP violations**: switch statements that should be polymorphic
  dispatch (e.g., per-strategy dispatch in ZSTD encoder).
- **MECE violations**: overlapping responsibilities between modules
  (e.g., bit writers in multiple crates).
- **DRY violations**: copy-pasted code (e.g., CRC-32, adler-32,
  match finders).
- **Naming inconsistencies**: e.g., `deflate_fixed_huffman` vs
  `deflate_dynamic_huffman` vs `deflate_stored` — fine — vs `compress`
  in some crates and `encode_stream` in others — inconsistent.

## Proposed fix

1. Sweep each crate's `src/lib.rs` for naming inconsistencies.
2. Find every `match params.strategy` or `match params.kind` and
   evaluate whether it could be trait dispatch.
3. Find every `Vec<u8>` allocation in a hot loop and check if a
   reusable buffer would help.
4. Document findings in `docs/code-review-{date}.md`.
5. Fix the high-impact items; file TODOs for the rest.

## Acceptance criteria

- [ ] Review document committed.
- [ ] High-impact fixes land.
- [ ] Remaining items filed as TODOs.

## Priority

P2 — pure code-quality work.
