# 01 — ZSTD compressed literals decode

**Status**: ✅ Partial — direct-encoded weights (iSize ≥ 128) work;
FSE-compressed weights (iSize < 128) blocked on sliding-window
`BitStream` (see task [02]).

## What was fixed

Three bugs from `BUGREPORT-zstd-0.1.0.md`:

1. **BUG 1 (block_type bits)**: `src/literals/mod.rs:75` — was
   `(header0 >> 6) & 0x03`, now `header0 & 0x03`. Verified against
   `zstd_decompress_block.c:65` (`istart[0] & 3`).
2. **BUG 2 (size_format)**: `src/literals/mod.rs:98-122` — was using
   bit 0 as the discriminator, now uses bits 2-3 as a 2-bit `lhlCode`
   matching the C reference switch.
3. **BUG 3 (compressed literals)**: `src/literals/mod.rs:167+` and
   `src/huffman/weights.rs` — implemented header parse + Huffman table
   read (direct encoding only) + single-stream and 4-stream literal
   decode.

## Verified by

- `cargo test -p omnizip-zstd literals`
- `cargo test --test zstd_parity` — 7/11 fixtures pass (was 4/11).
  Includes the previously-broken `test-]he[`, `zeroSeq_2B`,
  `block-128k`.

## Remaining

FSE-compressed weights (iSize < 128) — the NORMAL case for any
non-trivial ZSTD frame — needs task [02] (sliding-window BitStream).

`huffman-compressed-larger.zst` still skips with `Unsupported:
FSE-compressed huffman weights need a sliding-window BitStream (TODO)`.
