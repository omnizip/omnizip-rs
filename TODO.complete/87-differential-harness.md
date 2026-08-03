# 87 — Differential parity harness vs C/Ruby references

**Status**: ✅ Resolved — 2026-08-03.

**Priority:** High
**Source:** CLAUDE.md (release blocker per existing TODO)

## Context

`TODO.omnizip-rs/02-cross-language-differential-harness.md` is still
open. Without it, we can't prove correctness against reference
implementations. The `tests/differential/` skeleton exists but isn't
wired to CI.

After our recent LZMA xz interop fix (PR #36) we have proven the
encoder works against `xz -d`. The harness should generalize this:
every codec should round-trip via its reference decoder.

## Plan

For each codec with a reference implementation:

| Codec  | Reference       | Test mode                            |
|--------|-----------------|--------------------------------------|
| LZMA   | `xz` CLI        | Rust encode → `xz -d` → assert match |
| ZSTD   | `zstd` CLI      | Rust encode → `zstd -d` → assert match |
| BZip2  | `bzip2` CLI     | Rust encode → `bzip2 -d` → assert match |
| DEFLATE| `gzip`/`zlib`   | Rust encode → `gzip -d` → assert match |
| Brotli | `brotli` CLI    | Rust encode → `brotli -d` → assert match |
| Snappy | `snappy` CLI    | Rust encode → `snappy -d` → assert match |
| LZ4    | `lz4` CLI       | Rust encode → `lz4 -d` → assert match |
| FLAC   | `flac` CLI      | Rust encode → `flac -d` → assert match |

Skip PPMd/GLZA/ZPAQ/Rice++/FSST/BLOSC/Deflate64 (no canonical CLI
or our format is custom).

## CI integration

Tests run via `cargo test --test differential`. Each test:
1. Check that the reference binary is installed (skip if not).
2. Encode test fixture via Rust.
3. Pipe bytes to reference decoder.
4. Compare output to original input.

CI installs all reference CLIs in `.github/workflows/differential.yml`.

## Acceptance criteria

- [ ] `tests/differential/` has one test file per codec above.
- [ ] All tests pass locally with reference CLIs installed.
- [ ] CI workflow added that installs CLIs and runs the suite.
- [ ] Documentation in `tests/differential/README.md`.

## Files

- `tests/differential/` — already exists; populate with test files
- `.github/workflows/differential.yml` — CI integration
- `tests/differential/README.md` — how to run locally

## Sequencing

- **Do first** — this is the foundation that #83, #84, and any future
  codec work needs. Without it, regressions ship silently.
- The omnizip-bench crate (TODO 86, ✅) provides the determinism
  check but not the cross-language parity. Both are needed.

## Effort estimate

- 3-5 days for the core harness (8 codecs × 1 test file each).
- +1-2 days for CI integration (`.github/workflows/differential.yml`
  needs `apt install xz-utils zstd bzip2 gzip brotli libsnappy-dev
  liblz4-tool flac` on Ubuntu runners).

## Risks

- Some CLIs add framing that our Rust decoders don't expect (e.g.
  `gzip` headers, `bzip2` magic). Handle by either (a) using the
  "raw" format on both sides, or (b) parsing the framing in the
  test.
- macOS vs Linux CLI quirks (BSD vs GNU). Pin specific CLI versions
  in CI to avoid drift.
- Reference CLIs are not always installed on dev machines. Document
  install instructions per-platform in `tests/differential/README.md`.

## References

- CLAUDE.md — original spec for the differential gate.
- `tests/differential/ruby_reference/` — existing Ruby-runner
  scaffolding (used by the LZMA xz interop work).

