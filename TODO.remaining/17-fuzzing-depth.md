# 17 — Fuzzing depth

- **Priority:** MEDIUM (hardening; issue #315 was fuzz-found)
- **Depends on:** [TODO.omnizip-rs 31](../TODO.omnizip-rs/31-fuzz-targets.md)
- **Estimated effort:** 2 days
- **Status:** pending

## Goal

Implement TODO 31: one cargo-fuzz target per decoder plus
encode-round-trip targets, seeded from the conformance corpus, run
nightly in CI — plus an always-on, stable-toolchain structured
mutation gate so every PR exercises malformed-input paths.

## Deliverables

1. `fuzz/` crate: targets for lzma2/xz/lzma-alone, zstd, deflate,
   bzip2, brotli, ppmd7, bcj filters (and rar/zip archive parsers if
   cheap to add). Decoder targets assert no panic, no infinite loop
   (`-timeout=10`), errors fine.
2. Round-trip targets: encode arbitrary input at fixed levels →
   decode → byte-compare. Catches encoder bugs that emit invalid
   streams.
3. Seed corpora from `tests/` fixtures + `~/sweep-corpus/` files.
4. `.github/workflows/fuzz.yml` nightly: 5 min/target, artifacts
   uploaded on crash.
5. **Stable gate** (no nightly dep, runs in normal CI):
   `tests/fuzz_smoke/` — seeded RNG mutates valid streams (bit flips,
   truncations, length-field corruption, table-count corruption),
   decoders must return errors not panics. Deterministic seeds so
   failures reproduce.

## Rules

- Crash artifacts root-caused become committed regression tests
  before the target is re-enabled.
- Fuzz crate lives OUTSIDE the workspace lints surface where needed
  (libfuzzer harness requires nightly; keep workspace on stable —
  check the root Cargo.toml excludes `fuzz/` from members).
- Findings that are "rejects cleanly" are still worth keeping as
  corpus seeds.

## Acceptance

- All targets built and running nightly; fuzz-smoke test green in
  normal CI on linux + macOS.
- At least one sustained local run (30 min/target on the codec
  decoders) with zero panics, or all findings fixed + regression
  tests committed.
