# 172: Brotli Decoder — Full RFC 7932 Support

## Priority: P3 (only blocks decoding reference brotli streams)

## Status: partial — NPOSTFIX/NDIRECT landed; multi-block-type, context
maps, static dictionary still pending.

## What landed (2026-08-06)

### NPOSTFIX + NDIRECT distance codes (PR #127 — feat/brotli-npostfix-ndirect)

`decode_distance_from_code` now mirrors upstream `ReadDistanceInternal`:
- `dist_code < 16`           → short code via dist_rb.
- `dist_code < 16 + NDIRECT`  → direct code: distance = code - 15.
- `dist_code >= 16 + NDIRECT` → long code with NPOSTFIX postfix bits
  + nbits extra bits from `(distval >> 1) + 1`.

This unlocks metablocks where the encoder needs NPOSTFIX > 0 or
NDIRECT > 0 (longer-distance-heavy inputs). For the trivial case
(NPOSTFIX=0, NDIRECT=0) the formula collapses to the previous fast
path, so all 71 existing brotli tests still pass.

## What remains

### Multi-block-type machinery (RFC 7932 §9.3)

- `NBLTYPESL/C/D` up to 256 via `DecodeVarLenUint8` (already supported).
- Per-category block-type code (3 Huffman trees: ISIZE = 2 + NBLTYPES).
- Block-switch commands mid-metablock with block-length tracking.
- Per-block-type `ContextMode` selection.

### Context maps (RFC 7932 §9.6 + §10)

- Literal context map: `NTREESL` Huffman trees, with run-length +
  inverse MTF RLE decoding (`kContextMapRleAlphabet`).
- Distance context map: `NTREESD` Huffman trees, same RLE encoding.
- 2-bit `ContextMode` per literal block type (LSB6, MSB6, UTF8, SIGNED).
- `kUTF8ContextLookup` (256+256 entries) for UTF-8 mode.
- `kSigned3BitContextLookup` (256+256 entries) for SIGNED mode.

### Distance format extensions

- Already land — NPOSTFIX > 0 and NDIRECT > 0 are now decoded correctly.

### Static dictionary (RFC 7932 §10.3)

- `kBrotliDictionary` (≈ 1 MiB, ported from upstream — copyright clean)
- `kBrotliDictionaryOffsetsByLength` (32 entries).
- `kBrotliDictionarySizeBitsByLength` (32 entries).
- 121 `kTransforms`.
- Branch: `if distance_code > max_distance → dictionary lookup`.

### State machine

Upstream's `ProcessCommandsInternal` is a 13-state machine with
sub-states for huffman/context_map/tree_group. A full port is ~3–5K
LOC of careful Rust. It can be ported by translating the upstream
state enum + each state's body directly.

## Approach

If/when prioritised:
1. Port `kUTF8ContextLookup`, `kSigned3BitContextLookup` (read-only
   constant tables — easy).
2. Add `HuffmanTreeGroup` struct (vector of HuffmanTable per category).
3. Port `BrotliReadContextMap` (the RLE + inverse-MTF state machine).
4. Port `BrotliDecodeBlockTypeSwitch` (block-type code reading).
5. Refactor `decode_compressed_metablock` to take a tree group +
   context map + context mode instead of single tables.
6. Port static dictionary + transforms.
7. Add fixture-based differential test against `brotli -d` on real
   files.

## Acceptance Criteria

- Decode all 11 brotli fixtures from upstream's test corpus.
- Decode every `.br` produced by `brotli -q 1` through `brotli -q 11`
  on the Linux kernel source (or similar large text).
- Differential test: decode 1000 random inputs through our decoder and
  `brotli -d`, assert byte-identical output.
