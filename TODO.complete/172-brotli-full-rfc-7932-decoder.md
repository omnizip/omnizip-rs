# 172: Brotli Decoder — Full RFC 7932 Support

## Priority: P3 (only blocks decoding reference brotli streams)

## Status: pending

## Context

Our decoder currently handles only the trivial layout emitted by our
own `compress_fragment_two_pass` encoder:

- NBLTYPESL = NBLTYPESC = NBLTYPESD = 1
- NPOSTFIX = 0, NDIRECT = 0
- NTREESL = NTREESD = 1 (no context maps)
- CONTEXT_MODE = LSB6 (unused under trivial literal context)

The encoder produces this layout on every input, and our decoder
round-trips it. But the decoder cannot consume reference brotli files
from `brotli -q 11` or third-party tools, which use the full grammar.

## What needs to land

### Block-type machinery (RFC 7932 §9.3)

- `NBLTYPESL`, `NBLTYPESC`, `NBLTYPESD` up to 256 via `DecodeVarLenUint8`.
- Per-category block-type code (3 Huffman trees: ISIZE=2+NBLTYPES×…).
- `MLEN - IBLEN` accounting across block boundaries.
- Block-switch commands mid-metablock.

### Context maps (RFC 7932 §9.6 + §10)

- Literal context map: NTREESL explicit, with run-length + inverse
  MTF RLE decoding.
- Distance context map: NTREESD explicit, same RLE encoding.
- `ContextMode` per literal block type (LSB6, MSB6, UTF8, SIGNED).
- Context lookup for UTF-8 (`kUTF8ContextLookup`) and SIGNED
  (`kSigned3BitContextLookup`).

### Distance format (RFC 7932 §9.4)

- `NPOSTFIX` ∈ 0..3 and `NDIRECT` ∈ 0..15 (with the
  `if NDMOEM < 12 → NDIRECT = NDMOEM; else NDIRECT = (NDMOEM−12) << NPOSTFIX` formula).
- Direct + postfix distance codes.
- Complex distance formula with postfix bits.

### Static dictionary (RFC 7932 §10.3)

- Detect `distance_code > max_distance` → dictionary lookup.
- Use `kBrotliDictionary`, `kBrotliDictionaryOffsetsByLength`,
  `kBrotliDictionarySizeBitsByLength` from upstream.
- All 121 transforms (`kTransforms`).

## Approach

Port upstream `brotli-decompressor`'s state machine structure:

1. `BrotliState` struct with substate enums for huffman, context_map,
   tree_group, uncompressed, command.
2. `ProcessCommandsInternal` becomes a `loop { match s.state {…} }`
   that can suspend/resume across input chunks.
3. Per-category Huffman tree groups (`HuffmanTreeGroup` in upstream).

This is a substantial port — 3–5K LOC of state-machine Rust. Defer until
we have a real consumer that needs reference-brotli decode parity. The
omnizip-rs registry only needs round-trip parity for LimniFS content
addressing, which we already have.

## Acceptance Criteria

- Decode all 11 brotli fixtures from upstream's test corpus.
- Decode every `.br` produced by `brotli -q 1` through `brotli -q 11`
  on the Linux kernel source.
- Differential test: decode 1000 random inputs through our decoder and
  `brotli -d`, assert byte-identical output.
