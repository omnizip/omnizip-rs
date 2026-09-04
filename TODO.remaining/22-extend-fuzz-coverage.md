# 22 — Extend fuzz coverage to the remaining decoders

- **Priority:** MEDIUM
- **Depends on:** [17](17-fuzzing-depth.md)
- **Status:** done 2026-09-04

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


## Results (2026-09-04) — gate extended to 15 codecs, 1 real bug found

The extended gate (ppmd7/ppmd8, deflate64, flac, fsst, glza, zpaq,
ricepp, blosc added; slow statistical codecs at reduced case counts;
~55s debug, ~9s release) found a real **deflate64 ENCODER** bug on
its first run:

- `DISTANCE_TABLE` entry 29 was `(32769, 13)`, leaving distances
  24 577..32 768 with no bucket — `distance - base_d` underflowed in
  debug and silently emitted the WRONG distance code in release. The
  extension comment's arithmetic was also wrong (16 385 + 8 191 =
  24 576, not 65 536). Fixed to `(24 577, 13)` (the Ruby reference's
  decoder table, also standard-deflate-consistent) and `MAX_DISTANCE`
  capped to the encodable 32 768. Deflate64 suite 17/17.
- cargo-fuzz: ppmd7/ppmd8/deflate64 decode-no-panic targets added;
  nightly matrix now 13 targets.

### Follow-up probe (small)

The Deflate64 64K extension is unverified against a reference
extractor — the Ruby encode side maps 32 769..65 536 to code 29,
unrepresentable in 13 extra bits (the Ruby is inconsistent there
too). Craft a Deflate64 zip (method 9, large-window content) with a
reference tool, extract with `7zz`, pin the true 64K distance layout,
then extend the table + MAX_DISTANCE accordingly.
