# TODO 136: libdeflate — remove `miniz_oxide` fallback

## Problem

`omnizip-libdeflate` has its own in-house encoder (stored + fixed +
dynamic Huffman) and decoder, but `Cargo.toml` still pulls in
`miniz_oxide` for the decoder fallback path.

## Proposed fix

The fallback was added during Phase 2 stabilisation. With the
in-house decoder now round-tripping all test fixtures, the fallback
is dead weight:

1. Remove the `miniz_oxide::inflate::decompress_to_vec_zlib` and
   `decompress_to_vec` fallback calls in
   `omnizip-libdeflate/src/lib.rs::LibdeflateCodec::decompress`.
2. Remove `miniz_oxide` from `Cargo.toml`.
3. Verify all tests still pass.

## Acceptance criteria

- [ ] No `miniz_oxide` in `omnizip-libdeflate/Cargo.toml`.
- [ ] All 24 libdeflate tests pass via in-house decoder only.
- [ ] Differential parity preserved against `gzip -d`.

## Priority

P1 — eliminates the last external dep in a "pure-Rust" codec.
