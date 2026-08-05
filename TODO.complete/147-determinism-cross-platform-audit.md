# TODO 147: Determinism audit — cross-platform

## Problem

LimniFS requires byte-identical output across runs, machines, and
Rust versions. We assert this in tests but haven't audited:

- `f64` arithmetic in FLAC LPC across platforms (IEEE 754 should be
  deterministic, but `powi` / `log2` / `sin` etc. may differ).
- `HashMap` iteration (we use `BTreeMap` in deterministic paths;
  other uses?).
- `DefaultHasher` usage (none in encode paths; verify).
- `std::time::Instant` seeding (none in encode paths; verify).
- Floating-point rounding modes (default is round-to-nearest; verify
  no fast-math).

## Proposed fix

1. Audit every `as f64`, `f64::sqrt`, `f64::log2`, etc. in encode
   paths. Document that they're stable across platforms.
2. Add cross-platform determinism tests:
   - Encode fixtures, hash the output, commit hashes to repo.
   - CI verifies the hashes match across linux + macOS + Windows.
3. Document any platform-dependent code that we can't avoid.

## Acceptance criteria

- [ ] Determinism audit document in `docs/determinism.md`.
- [ ] Cross-platform hash-based tests in CI.
- [ ] Every f64 operation in encode paths catalogued.

## Priority

P1 — LimniFS's dedup depends on this. A single platform-dependent
codec would break content addressing.
