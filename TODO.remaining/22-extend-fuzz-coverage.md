# 22 — Extend fuzz coverage to the remaining decoders

- **Priority:** MEDIUM
- **Depends on:** [17](17-fuzzing-depth.md)
- **Status:** pending 2026-09-04

## Goal

The smoke gate (`tests/fuzz_smoke`) and the nightly cargo-fuzz matrix
cover brotli/zstd/xz(lzma)/deflate/bzip2/lz4/snappy. The workspace
ships more decoder surfaces that malformed input can reach:

- `omnizip-ppmd` (PPMd7/8 — also reachable through RAR and 7z
  containers, the highest-exposure uncovered decoder)
- `omnizip-deflate64` (zip)
- `omnizip-flac`, `omnizip-fsst`, `omnizip-glza`, `omnizip-zpaq`,
  `omnizip-ricepp`, `omnizip-blosc`
- archive parsers as a follow-up (rar/zip/7z/tar readers take
  attacker-controlled bytes; the RAR4 corrupt-fixture tests cover a
  slice)

## Work

1. Add every crate exposing `Codec::decompress` to the smoke gate's
   codec list (encode at a fast level, mutate, decode).
2. Add decode-no-panic cargo-fuzz targets for ppmd and deflate64 at
   minimum (container-reachable).
3. Any panic found → fix + committed regression case (the task-17
   pattern: 16 panics found and fixed on day one).

## Acceptance

- Gate covers every decoder crate; green in CI.
- A 30-minute local libFuzzer pass over ppmd/deflate64 with zero
  panics, or all findings fixed.
