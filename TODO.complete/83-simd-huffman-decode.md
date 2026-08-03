# 83 — SIMD Huffman decode

**Priority:** Medium
**Source:** RESEARCH.md §4 (SIMD in Rust 2025)

## Context

Huffman decoding is the inner loop of every DEFLATE-derived codec:
DEFLATE, Deflate64, libdeflate, Brotli (literal+length+distance
tables), ZSTD (Huffman literal decoder), BZip2 (Huffman-coded MTF).

Standard table-driven decode: peek N bits, index into 2^N table,
consume code length, output symbol. Sequential dependency on the
"consume" step kills auto-vectorization.

## Realistic improvement

True parallel Huffman decode is hard (variable-length codes). The
practical SIMD win is in the **table-lookup phase**:

1. Stream 8 codes from the bitstream into a `u8x8` vector.
2. Gather 8 symbols via `simd::simd_gather` (when stable) or manual
   unrolled lookups.
3. Write 8 symbols to output in one `store`.

Expected gain: 2-3x on the decode inner loop. Less dramatic than
CRC32 SIMD but still meaningful for codec-heavy workloads.

## Pre-reqs

- `std::simd::Simd<u8, N>` gather is still nightly-only on most
  Rust stable toolchains. May need to wait for Rust 1.85+ or use
  portable `wide` crate.

## Acceptance criteria

- [ ] Profile-guided: identify which codec benefits most.
- [ ] Implement SIMD Huffman decode in that codec first (likely
      DEFLATE since miniz_oxide is the existing baseline).
- [ ] ≥1.5x throughput improvement on Enwik8.
- [ ] Workspace tests pass.

## Files

TBD — depends on which codec ships first.
