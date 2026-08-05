# TODO 156: Coverage measurement +Codec trait tests

## Problem

No coverage measurement. We have 940+ tests but don't know which
codec paths are uncovered.

## Proposed fix

1. Add `tarpaulin` or `llvm-cov` to CI.
2. Upload coverage to `codecov.io`.
3. Per-PR coverage diff comment.
4. Maintain ≥ 80% coverage on codec crates.

## Acceptance criteria

- [ ] Coverage report generated on every PR.
- [ ] Coverage badge in workspace README.
- [ ] Each codec crate ≥ 80% line coverage.
- [ ] Per-crate uncovered paths documented in `docs/coverage.md`.

## Priority

P2.
