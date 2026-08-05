# TODO 120: Continuous differential parity harness

## Problem

The differential harness (`tests/differential/`) currently runs
on-demand via `cargo test`. For a release-quality workspace, it
should run continuously against:

1. The Ruby omnizip reference (pinned SHA in `ruby-ref.txt`).
2. The C `xz-utils` / `zstd` / `flac` / `bzip2` reference binaries.
3. The upstream `brotli` reference (until TODO 117 lands).
4. Property-based random inputs (via `proptest`).

Currently no CI integration; the harness only catches regressions
when someone remembers to run it.

## Proposed fix

1. Add a GitHub Actions workflow `differential.yml` that runs the
   harness on every PR touching `omnizip-{lzma,zstd,flac,bzip2,...}`.
2. Pin reference SHAs in `tests/differential/{ruby-ref.txt,
   xz-ref.txt, zstd-ref.txt, ...}` so a reference update is an
   intentional act.
3. Add property-based tests with `proptest`: generate random inputs
   of varying compressibility, encode via each codec, decode via
   reference, compare byte-exact.
4. Add a "fuzz mode" that runs indefinitely with seed tracking, for
   finding rare edge cases.

## Acceptance criteria

- [ ] GHA workflow runs the differential harness on every PR.
- [ ] Reference SHAs pinned; updates are explicit PRs.
- [ ] At least one property-based test per codec.
- [ ] Fuzz mode available via `cargo run --example fuzz-differential`.

## Priority

P1 — without this, every encoder change risks a subtle regression
that goes unnoticed until production.
