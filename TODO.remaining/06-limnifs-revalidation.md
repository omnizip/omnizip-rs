# Task 06: Downstream LimniFS re-validation

## Status: done (2026-08-29) — validated against 0.21.23

## What ran

- Local checkout `~/src/limnifs/limnifs` (main): pinned
  `omnizip-*` 0.21.20 → 0.21.23 in `limnifs-core/Cargo.toml` +
  `limnifs-write/Cargo.toml`, `cargo update` for the omnizip
  packages (libdeflate/lz4/brotli/bzip2/codecs/deflate/deflate64/
  filters/flac/fsst/glza/blosc).
- `cargo test --workspace`: **EXIT=0, 27 suites ok, 649 passed,
  0 failed.**
- No hangs. The two slowest suites (whole-file drop categorizer on
  slabs) take ~4 minutes each and complete normally — that is corpus
  work, not a #388-style stall.

The pin bump is left UNCOMMITTED in the local LimniFS working tree
(downstream pushes are the owner's call).

## Acceptance

- [x] limnifs-core builds against the latest omnizip (0.21.23,
      newer than the 0.21.18 the task file asked for)
- [x] All tests pass, no hangs
