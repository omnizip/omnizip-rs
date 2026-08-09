# 234 — Shared Bitstream Module

- **Priority:** P3 (architecture — DRY across codecs)
- **Crate:** `omnizip-codecs` (shared module)
- **Depends on:** [233](233-shared-match-finder-abstraction.md)
- **Estimated effort:** 2 days

## Goal

Extract the LSB-first bit writer and reader from individual codec crates
into a shared `bitstream` module in `omnizip-codecs`. Multiple codecs
currently have their own BitWriter implementations.

## Background

- `omnizip-brotli::encoder::bitwriter::BitWriter`
- `omnizip-zstd` (inline bit writing in various places)
- `omnizip-codecs::bitstream` (partial — exists but not adopted everywhere)

All implement the same LSB-first bit packing:
- Accumulator holds partial bits
- write_bits appends bits LSB-first
- byte_align flushes to byte boundary
- flush writes remaining accumulator bytes

## Plan

1. Consolidate BitWriter into `omnizip-codecs::bitstream::BitWriter`
2. Add BitReader (already in Brotli's decoder — extract to shared)
3. Update all codecs to use the shared implementation
4. Add comprehensive property tests for bit-level correctness

## Acceptance criteria

- [ ] Single BitWriter/BitReader in `omnizip-codecs::bitstream`
- [ ] All codec-specific bit writers removed
- [ ] Property tests verify round-trip (write then read = identity)
- [ ] No performance regression (benchmark before/after)
