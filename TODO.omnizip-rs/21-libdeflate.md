# 21 — libdeflate

- **Priority:** P2 (faster DEFLATE)
- **Depends on:** [16](16-deflate.md)
- **Estimated effort:** 2 weeks
- **Crate:** `omnizip-deflate` (extended) or new `omnizip-libdeflate`

## Why

libdeflate (Eric Biggers 2016) is a high-performance DEFLATE/zlib/gzip
implementation: 2–3x faster than zlib with better ratio. Used in the Linux
kernel, PostgreSQL, and modern compression tooling. LimniFS users who want
the gzip-interop tier should use libdeflate, not vanilla DEFLATE.

The C reference is at `ebiggers/libdeflate` (MIT). A pure-Rust port does
not exist as of 2026; this task ports it.

## Approach

1. **Phase A** (1 week): port the DEFLATE decoder from libdeflate. It's a
   highly optimised Huffman + LZ77 decoder with table-driven state
   machines. Port the tables verbatim.
2. **Phase B** (1 week): port the DEFLATE encoder. libdeflate uses a
   greedy match finder with Huffman code reuse. Simpler than the decoder.

The output is standard DEFLATE — byte-identical to zlib at the same level.
The advantage is speed.

## Acceptance

- **Decode parity:** every `.gz` / `.zlib` fixture decompresses byte-identically
  to `gzip -d`.
- **Encode parity:** Rust encoder output at level 1–12 decompresses
  byte-identically through `gzip -d`.
- Ratio within 2% of `gzip -9` on Silesia.
- Decode throughput ≥ 1 GB/s on Apple M1 single core (libdeflate's C target
  is ~1.5 GB/s; Rust should match within 30%).
- Encode throughput ≥ 200 MB/s at level 6.
- Clippy clean, no `unsafe`, deterministic.

## Implementation notes

- libdeflate's speed comes from table-driven Huffman decode (lookup tables
  instead of tree walks). Port the table generation carefully.
- The encoder's match finder uses a hash chain with depth 1–32 depending on
  level. Simpler than LZMA's match finder.
- This crate is distinct from `omnizip-deflate` (task 16, the
  Ruby-port). `omnizip-deflate` is the spec-faithful Ruby port;
  `omnizip-libdeflate` is the speed-optimised port. Both produce
  standard DEFLATE.
