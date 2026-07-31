# 22 — LZ4 HC (high compression)

- **Priority:** P2 (higher-ratio LZ4 variant)
- **Depends on:** [01](01-codec-trait-registry.md)
- **Estimated effort:** 3 days
- **Crate:** `omnizip-lz4` (new crate, or extend limnifs's existing lz4 path)

## Why

LZ4 HC (High Compression) is the same LZ4 format with a more thorough
match finder. It produces 2–3x better ratio than LZ4 fast mode at the cost
of 5–10x slower encode. Decode is identical speed (same format).

For LimniFS users who want LZ4's decode speed but better ratio on archival
drops, LZ4 HC fills the gap between LZ4 and ZSTD.

## Approach

`lz4_flex` (the crate limnifs already uses) supports both fast and HC
modes via `compress_prepend_size` vs `compress_high`. Wrap the HC variant
as a separate codec.

No porting from omnizip Ruby required — `lz4_flex` is the standard
pure-Rust LZ4 and supports both modes.

## Acceptance

- LZ4 HC codec registered in `omnizip-codecs::CodecRegistry`.
- Round-trips on every corpus fixture.
- Output decompresses byte-identically through reference `lz4 -d`.
- Ratio within 10% of reference `lz4 -9` on Silesia.
- Decode throughput ≥ 1 GB/s (same format as fast LZ4).
- Encode throughput ≥ 30 MB/s at level 9.
- Clippy clean, no `unsafe`, deterministic.

## Implementation notes

- The codec id differs from fast LZ4 — they're separate codec entries in
  the registry.
- LZ4 HC and LZ4 fast share a decoder; the difference is encoder only.
