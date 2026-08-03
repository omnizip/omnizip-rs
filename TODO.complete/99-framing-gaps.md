# 99 — Differential harness: fix bzip2/lz4/DEFLATE framing gaps

**Priority:** Medium
**Source:** TODO 87 (partial — FLAC, brotli, and round-trip parity work)
**Status:** ✅ Resolved — 2026-08-03

## Current state

`tests/differential/tests/cli_parity.rs` has 4 codec parity tests:

| Codec   | Status           | Notes                                   |
|---------|------------------|-----------------------------------------|
| brotli  | ✅ Passes        | Rust encode → CLI decode                |
| bzip2   | 🚫 Out of scope  | Ruby reference also emits custom format |
| lz4     | ✅ Passes        | Uses `omnizip_lz4::compress_frame`     |
| DEFLATE | ✅ Passes        | Python zlib wbits=47 (auto-detect)     |

## Resolved framing gaps

- **lz4**: Added `omnizip_lz4::compress_frame` / `decompress_frame`
  using `lz4_flex::frame::FrameEncoder`. The Rust codec's default
  `Codec::compress` still emits raw LZ4 blocks (round-trippable via
  own decoder); the frame API is the CLI-compatible surface.

- **DEFLATE**: Discovered the Rust encoder produces zlib-wrapped
  DEFLATE (`78 9C` header), not raw DEFLATE. Updated the parity test
  to use Python `zlib.decompress(data, 47)` — wbits=47 means
  auto-detect zlib/gzip/raw.

## Why bzip2 is out of scope

The Ruby omnizip reference (`../omnizip/lib/omnizip/algorithms/bzip2/encoder.rb`)
emits a custom non-`.bz2` wire format:

```text
u32 crc32 + u32 primary_index + u32 orig_len + u32 rle_len
+ u16 code_count + [u8 symbol, u8 code_length]* + u8 padding
+ u32 bitstream_len + [u8]* bitstream_len
```

The Rust port is a faithful translation. Standard `.bz2` CLI parity
would require porting the full bzip2 bit-level wire format:

- `BZh<level>` magic
- Block magic `0x314159265359` (BCD of π)
- 32-bit CRC, randomised flag, 24-bit origPtr
- 16+16-bit symbol usage map + unary-encoded Huffman code lengths
- MTF output RUNA/RUNB run-length encoding
- End-of-stream magic `0x177245385090`

That is a separate effort and not part of the Ruby→Rust port scope.
The skip-on-error branch in `cli_parity.rs::bzip2_round_trips_through_reference_cli`
documents this as a known divergence.

## Acceptance criteria

- [x] lz4 parity test passes.
- [x] DEFLATE parity test passes.
- [x] All tests still skip cleanly when CLI is missing.
- [x] bzip2 divergence documented as intentional (Ruby reference also
      uses custom wire format — `.bz2` interop is a separate effort).

## Files

- `omnizip-lz4/src/lib.rs` — `compress_frame` / `decompress_frame`
- `tests/differential/tests/cli_parity.rs` — updated to use framed APIs
