# TODO 135: Filters — replace `lz4_flex` with in-house LZ4

## Problem

`omnizip-filters` (2282 LOC) uses `lz4_flex` for one of its
filters. The crate is supposed to be a pure-Rust implementation of
the BCJ / Delta / etc. preprocessing filters.

## Proposed fix

Replace the `lz4_flex` calls with in-house LZ4 once TODO 132 lands.
Most filters don't need LZ4 at all (BCJ, Delta, ARM, etc.) — audit
and remove the dep if it's only used in one filter.

## Acceptance criteria

- [ ] No `lz4_flex` in `omnizip-filters/Cargo.toml`.
- [ ] All filter tests pass.

## Priority

P2.

## Dependencies

- TODO 132.
