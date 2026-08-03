# 82 — SIMD-accelerated CRC-32 and XXHash-64

**Priority:** High — 🔄 **CRC-32 DONE (slice-by-8); PCLMULQDQ + XXHash-64 PENDING**
**Source:** RESEARCH.md §4 (SIMD in Rust 2025, zlib-rs case study)

## Status

**CRC-32** landed in `omnizip-codecs/src/checksum.rs` as a shared
slice-by-8 implementation. 9 unit tests pass, including differential
vs byte-by-byte and known-value checks against Python's
`zlib.crc32`.

- Slice-by-8 processes 8 bytes per loop iteration via 8 parallel
  table lookups. ~3× faster than byte-by-byte on inputs > 1 KB due
  to instruction-level parallelism.
- `pub fn crc32_iso_hdlc(&[u8]) -> u32` is the public entry point.
- Three existing per-crate CRC-32 implementations (omnizip-bzip2,
  omnizip-deflate, omnizip-lzma) are now DRY candidates — migrate
  them to the shared impl in a follow-up.

**True SIMD (PCLMULQDQ) is still blocked.** It requires
`core::arch::x86_64::_mm_clmulepi64_si128` which is `unsafe`, and
`#![forbid(unsafe_code)]` is workspace-wide. A future opt-in
`unsafe-simd` cargo feature could gate this — documented in the
module.

**XXHash-64** is still scalar in `omnizip-zstd/src/xxhash.rs`.
SIMD-accelerating it has lower ROI than CRC-32 (XXHash is already
fast) and is deferred.

## Context

CRC-32 is the workhorse checksum for XZ, gzip, BZip2. XXHash-64 is
the ZSTD frame checksum. Both are CPU-bound and trivially
SIMD-vectorizable.

Current implementations are scalar table-lookup. Using `std::simd`
we can get 4-16x throughput, which matters because:

- ZSTD encodes/decoders call XXHash on every block
- XZ verifies CRC-32 on every block
- gzip CRC is in the inner loop

`#![forbid(unsafe_code)]` blocks raw intrinsics, so the path is
`std::simd` (portable SIMD) only.

## Implementation plan

1. **CRC-32 (zlib polynomial 0xEDB88320)**:
   - PCLMULQDQ-style approach: process 16 bytes at a time using
     `std::simd::u64x2` and barrett reduction.
   - Portable SIMD doesn't expose PCLMULQDQ directly; use the
     "slice-by-N" fallback (8 or 16 bytes per iteration via
     table lookups in parallel).
   - Reference: zlib-rs `crc32_simd.rs`.

2. **XXHash-64**:
   - 32-byte stripe processing via `u64x4`.
   - Accumulate, then fold at end.
   - Reference: zstd's `xxh3` SIMD path.

## API

Replace existing free functions with new SIMD-aware versions behind
runtime CPU detection:

```rust
pub fn crc32(data: &[u8]) -> u32;  // dispatches at runtime
pub fn xxhash64(data: &[u8], seed: u64) -> u64;
```

Add `simd` cargo feature (default on for std targets, off for
no_std). Performance benchmark in `omnizip-bench` (see TODO 86).

## Acceptance criteria

- [ ] `crc32` SIMD variant: ≥3x throughput on ≥1 KB inputs.
- [ ] `xxhash64` SIMD variant: ≥3x throughput on ≥1 KB inputs.
- [ ] Output byte-identical to scalar version (verified by
      differential test).
- [ ] Workspace tests pass.
- [ ] `#![forbid(unsafe_code)]` preserved.

## Files

- `omnizip-lzma/src/crc32.rs` — add SIMD path
- `omnizip-zstd/src/xxhash.rs` — add SIMD path
- New `omnizip-codecs/src/checksum/` module — shared if both crates
  use the same impl pattern
