# TODO 139: Per-codec property-based tests

## Problem

Unit tests cover known edge cases. Differential tests cover known
inputs. Neither catches the long tail of random-input bugs.

Recent regressions (TODO 110 ZSTD infinite loop, TODO 121 ZSTD L12+
output blowup) only triggered on specific input patterns. A
property-based fuzzer would have caught both before release.

## Proposed fix

Add `proptest` per codec. Properties to verify:

1. **Round-trip**: `decompress(compress(x)) == x` for any x.
2. **Length-stable**: `compress` output length is deterministic.
3. **Byte-determinism**: same input + level → byte-identical output.
4. **Level monotonicity** (loose): higher level → ≤ output size
   (with tolerance for small inputs where fixed overhead dominates).
5. **Round-trip via reference**: encode via Rust, decode via C/Ruby
   reference tool — byte-identical.

Each codec gets a `proptest` module covering at least 100 random
inputs per property.

## Acceptance criteria

- [ ] `proptest = "1.0"` added to dev-deps.
- [ ] At least LZMA, ZSTD, DEFLATE, LZ4 have property tests.
- [ ] `cargo test --workspace --features proptest` runs them.
- [ ] CI runs a 5-minute subset on every PR.

## Priority

P1 — direct regression prevention.

## Dependencies

- TODO 126 (fuzz testing) — overlaps.
