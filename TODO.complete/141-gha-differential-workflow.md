# TODO 141: GHA differential workflow

## Problem

Differential harness exists in `tests/differential/` but only runs
on demand. Regressions land when contributors forget to run it.

## Proposed fix

`.github/workflows/differential.yml` runs on every PR touching
`omnizip-{lzma,zstd,flac,bzip2,brotli,lz4,deflate,libdeflate,ppmd,zpaq}/src/**`:

1. Check out at the merge commit.
2. Build all codecs in release mode.
3. Clone `omnizip/omnizip` (Ruby ref) at the SHA pinned in
   `tests/differential/ruby-ref.txt`.
4. Install C `xz-utils`, `zstd`, `flac`, `bzip2`, `brotli`, `lz4`.
5. Run `cargo test --workspace --test differential`.
6. On failure, upload the failing fixtures as artifacts.

Same workflow runs nightly on `schedule` to catch reference-SHA
drift.

## Acceptance criteria

- [ ] Workflow file lands.
- [ ] Runs in < 10 minutes on a typical PR.
- [ ] Pin files (`ruby-ref.txt`, etc.) explicit; updates are
  separate PRs.
- [ ] Failure artifacts retained for 30 days.

## Priority

P1 — without CI, the differential harness is theatre.

## Dependencies

- TODO 120 (continuous differential harness) — overlap.
