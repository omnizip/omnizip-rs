# TODO 142: Benchmark regression detection

## Problem

Performance changes land without anyone noticing. The bench
harness exists but isn't compared against a baseline.

## Proposed fix

1. Commit a `bench-baseline.json` to the repo with the last known
   good per-codec throughput on a reference input.
2. Add `cargo run --example bench-regression` that compares the
   current run against the baseline, exits non-zero if any codec is
   > 10% slower.
3. GHA runs this on every PR touching codec code.
4. Updates to `bench-baseline.json` are explicit PRs with a
   justification comment.

## Acceptance criteria

- [ ] `bench-baseline.json` lands.
- [ ] `bench-regression` example runs cleanly.
- [ ] GHA workflow runs it on PRs.
- [ ] Documented policy for when to update the baseline.

## Priority

P2 — nice to have, but harder to make reliable than functional CI
(machine variance, etc.).
