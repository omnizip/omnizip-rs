# TODO 130: Brotli Phase B — Huffman + block-type decode

## Problem

TODO 117 Phase A landed in PR #89 (frame header + metablock header
+ bit reader). The decoder still can't actually decode — it parses
headers but bails on any block-type beyond empty metablocks.

Phase B adds:

1. **Block-type header** (RFC 7932 §9.3): BTYPE_*, NBLTYPESLIT,
   NBLTYPESEDIST, block-type context modes.
2. **Distance codes** (RFC 7932 §9.4): direct + complex forms.
3. **Huffman decoding** (RFC 7932 §9.5): prefix-code table reading
   from the bitstream, simple + complex forms.
4. **Context-mode literal decoding** (RFC 7932 §10): CONTEXT_LSB6,
   CONTEXT_MSB6, CONTEXT_UTF8, etc.

## Acceptance criteria

- [ ] `decoder.rs` adds `BlockTypeHeader`, `DistanceCode`,
  `HuffmanTable` types.
- [ ] Decoder can parse a real Brotli stream up to the point where
  literal/length/distance Huffman codes start.
- [ ] 30+ unit tests covering each new structure.
- [ ] Round-trips a "stored block" (UNCOMPRESSED) Brotli stream end
  to end.

## Priority

P0 — required for TODO 117 Phase C (full encoder).
