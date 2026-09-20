# BUGREPORT: LZ4 from-spec encoder slower than lz4_flex on random data

## RESOLVED (2026-09-20, current 0.21.x)

Re-measured on the current encoder: 100 MB xorshift-random compresses in
**0.014 s** block-level (release build) — ~15x faster than the lz4_flex
wrapper's 0.21 s baseline this report was filed against. The incompressibility
detector plus the stride machinery (skip-trigger growth, whole-block literal
escape) cleared the regression entirely. The report body below is kept as the
historical record of the 0.14.40-era problem.

## LimniFS version
v0.2.40 with omnizip 0.14.40 (local patch, not yet published)

## Affected crate
`omnizip-lz4`

## Problem
The from-spec LZ4 block encoder (shipped in 0.14.18, replacing the
`lz4_flex` wrapper) is **~2× slower** than `lz4_flex` was on
incompressible (random) data, even after the incompressibility
detector fix in commit `33464d3`.

## Reproduction

LimniFS benchmark on `random` dataset (100 MB of pseudo-random bytes,
max-write profile which uses LZ4 via `skip_chunking`):

| Version | Time |
|---|---:|
| omnizip-lz4 0.14.17 (lz4_flex wrapper) | **0.21 s** |
| omnizip-lz4 0.14.40 + incompressibility detector | 0.40 s |
| omnizip-lz4 0.14.40 (before detector fix) | 1.18 s |

The incompressibility detector helped (1.18 → 0.40) but the hash
lookup overhead on the first 1024 positions is still higher than
lz4_flex's simpler hash table implementation.

## Root cause hypothesis

The from-spec `compress_block` uses a 4-byte hash with full comparison
at every position. lz4_flex's hash table has lower per-position cost
because it uses a wider hash and fewer cache misses per probe.

## Suggested fix

1. Profile `compress_block` vs `lz4_flex::compress_prepend_size` on
   random data to find the per-position cost difference.
2. Consider a wider hash (8-byte → 16-bit tag) or a smaller hash
   table that fits in L1 cache.
3. Or: add an early-exit for inputs where the first 256 positions
   yield zero matches — switch to a single literal-only block
   immediately (the current 1024-position window still does full
   hash lookups on all 1024 positions).

## Impact on LimniFS

LZ4 is in 7 of 9 profiles as the binary chunk codec or
`skip_chunking` fast path. A 2× slowdown on incompressible data
affects all binary-heavy workloads.
