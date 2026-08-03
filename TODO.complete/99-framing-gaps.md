# 99 — Differential harness: fix bzip2/lz4/DEFLATE framing gaps

**Priority:** Medium
**Source:** TODO 87 (partial — FLAC, brotli, and round-trip parity work)

## Current state

`tests/differential/tests/cli_parity.rs` has 4 codec parity tests:

| Codec   | Status           | Notes                                   |
|---------|------------------|-----------------------------------------|
| brotli  | ✅ Passes        | libFLAC-style: Rust encode → CLI decode |
| bzip2   | ⚠️ Skip-on-error | "not a bzip2 file" — missing BZh magic  |
| lz4     | ⚠️ Skip-on-error | "Unrecognized header" — raw block vs frame |
| DEFLATE | ⚠️ Skip-on-error | "invalid stored block lengths"          |

Tests skip-on-error (not fail) so CI stays green. The framing gaps
are documented as known divergences.

## Root causes

- **bzip2**: `omnizip-bzip2` produces raw bzip2 stream without the
  `.bz2` file header (`BZh` magic + block-size flag).
- **lz4**: `omnizip-lz4` wraps `lz4_flex` which produces raw LZ4
  blocks, not LZ4 frames (magic `0x184D2204`).
- **DEFLATE**: `omnizip-deflate` wraps `miniz_oxide` which produces
  raw DEFLATE. Python's `zlib.decompress(data, -15)` should handle
  this but reports "invalid stored block lengths" — suggesting our
  encoder produces a non-standard stored block.

## Approach

For each codec, EITHER:
- (a) Add a thin framing layer in the codec crate (e.g.
  `omnizip-bzip2::compress_framed` that prepends `BZh` magic), OR
- (b) Document the raw-stream output as intentional and update the
  parity tests to wrap/unwrap framing before comparing.

Option (a) is cleaner (OCP: new function, no change to existing API).

## Acceptance criteria

- [ ] bzip2 parity test passes (not skip-on-error).
- [ ] lz4 parity test passes.
- [ ] DEFLATE parity test passes.
- [ ] All tests still skip cleanly when CLI is missing.

## Files

- `omnizip-bzip2/src/lib.rs` — add `compress_framed` / `decompress_framed`
- `omnizip-lz4/src/lib.rs` — add LZ4 frame wrapper
- `tests/differential/tests/cli_parity.rs` — update tests to use framed APIs
