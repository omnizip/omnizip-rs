# 32 — SIMD acceleration

- **Priority:** P2 (perf optimization, post-correctness)
- **Depends on:** [10](10-lzma-phase-a-decoder.md), [13](13-zstd-phase-a-decoder.md)
- **Estimated effort:** 2–3 weeks
- **Location:** per-crate `simd` feature

## Goal

Speed up hot paths using `std::simd` (portable safe SIMD). Targets:

- LZMA match finder's hash computation
- LZMA range coder's bit operations
- ZSTD Huffman decode table walks
- ZSTD sequence execution (memcpy with overlapping windows)
- DEFLATE / libdeflate Huffman decode
- BCJ filter byte-pattern matching

## Approach

Use `std::simd` (the portable SIMD API stabilised in Rust 1.75+). No raw
`unsafe` intrinsics. Each SIMD-optimised path is:

1. Behind a `simd` feature flag.
2. Benchmarked against the scalar path.
3. Falls back to scalar automatically on targets without the SIMD width.

## Phase scope

1. **Audit hot paths** (3 days): profile each codec with `perf record` /
   Instruments. Identify the top 3 hot paths per codec.
2. **LZMA match finder SIMD** (4 days): the hash chain probe is a tight
   loop over a `Vec<u32>`. SIMD-accelerate with `u8x16` gathers.
3. **ZSTD Huffman decode** (3 days): the table-walk is the hot path. Pack
   4 decode operations into a `u32x4`.
4. **DEFLATE inflate** (3 days): match copying is the hot path. SIMD-accelerate
   the overlapping `memcpy`.
5. **BCJ filters** (2 days): the byte-pattern match for branch instructions
   vectorises cleanly.

## Acceptance

- `simd` feature compiles on x86_64, aarch64, and wasm32.
- Decode throughput improves ≥ 30% on Apple M1 (NEON) and ≥ 50% on x86_64
  (AVX2) for LZMA and ZSTD.
- Output is byte-identical to the scalar path (deterministic).
- Fuzz targets pass with `simd` enabled.
- Clippy clean. The `simd` modules may use `#[allow(clippy::pedantic)]`
  sparingly where SIMD idioms don't match scalar patterns.

## Implementation notes

- `std::simd` is the right abstraction layer. Avoid `core::arch::x86_64` /
  `core::arch::aarch64` intrinsics unless `std::simd` can't express the
  operation (rare).
- SIMD code is harder to read. Each SIMD function carries a comment
  explaining the vectorisation strategy and linking to the scalar
  reference.
- Don't SIMD-optimise before the scalar path is verified correct. SIMD bugs
  are subtler than scalar bugs.
