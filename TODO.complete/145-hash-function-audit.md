# TODO 145: Hash function quality audit

## Problem

Each codec uses its own LZ77 hash function. They're all variants of
`multiplicative hash with prime 0x9E3779B1`, but with different
shift amounts and prime choices. No audit of collision rates on
real-world inputs.

A poor hash → more chain walking → slower encoder.

## Proposed fix

1. Audit current hash functions:
   - LZMA: `wrapping_mul(0x9E37_79B1) >> (32 - HASH_SHIFT)`.
   - ZSTD: `wrapping_mul(PRIME4_BYTES) >> (32 - h_bits)`.
   - LZ4: similar multiplicative.
   - libdeflate: similar.
2. Measure collision rates on:
   - English text (enwik8)
   - Binaries (linux kernel .text)
   - Sensor data (random + periodic)
3. Compare against alternatives:
   - FNV-1a
   - CityHash (truncated)
   - xxHash (truncated)
   -CRC32 (slow but high quality)
4. Pick the best per-codec and document.

## Acceptance criteria

- [ ] Audit report in `docs/hash-audit.md`.
- [ ] Collision-rate benchmarks checked in.
- [ ] Per-codec hash picked based on data, not history.

## Priority

P2 — perf optimization; current hash isn't broken, just maybe
sub-optimal.
