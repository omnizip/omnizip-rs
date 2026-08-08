# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Infinite loop in match finder backward extension (TODO 110) by @[object]

### Other

- Bump workspace to 0.15.0 by @[object]
- Apply rustfmt formatting across workspace by @[object]
- 0.14.20 — DEFLATE dynamic-Huffman correctness fix (TODO 116) by @[object]
- 0.14.19 — LZ4 bug fix + eliminate miniz_oxide/lz4_flex from deflate/blosc/filters by @[object]
- 0.14.18 — LZ4 from-spec (no lz4_flex dep) by @[object]
- 0.14.17 — LZMA ResetMode for cross-call warmup (TODO 165) by @[object]
- 0.14.16 — Snappy snap-compat (full wire-format compatibility) by @[object]
- 0.14.15 — Snappy from-spec encoder + LZMA match-finder reuse by @[object]
- 0.14.14 — ZSTD 7× encode speedup (TODO 152) + LimniFS TODOs 161-167 by @[object]
- 0.14.13 — CI workflows + determinism audit + Brotli Phase C by @[object]
- 0.14.12 — FLAC FFT autocorrelation + ricepp SIMD + Brotli dictionary by @[object]
- 0.14.11 — libdeflate pure-Rust + LzmaCompressor + TODOs 131-151 by @[object]
- 0.14.10 — ZSTD L12+ repetition regression fix by @[object]
- 0.14.9 — FLAC block-size pruning + shared match-finder + DEFLATE dynamic-Huffman + PpmdCompressor by @[object]
- 0.14.8 — ZSTD perf cliff fix + LPC + ricepp + LZ4 HC speedups by @[object]
- Deep LPC + ricepp + cross-codec match-finder wins by @[object]
- Encoder hot-path speedups (LZMA + FLAC + ZPAQ) by @[object]
