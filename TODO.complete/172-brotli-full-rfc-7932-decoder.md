# 172: Brotli Decoder — Full RFC 7932 Support

## Priority: P3

## Status: partial — ISLAST=1 metablock-header bug fixed; q=1..8 working for simple inputs.

## What landed (2026-08-06)

See TODO 174 for the detailed roadmap. Highlights:

- ✅ Distance formula for general NPOSTFIX + NDIRECT case.
- ✅ UTF-8 + SIGNED context lookup tables.
- ✅ `ContextMode::context_id_2(p1, p2)`.
- ✅ Full decoder scaffolding in `decoder_full.rs`: BlockTypeState,
  read_context_map, read_tree_group, decode_compressed_metablock_full.
- ✅ OCP dispatch from trivial fast path.
- ✅ **ISLAST=1 metablock-header fix** — bit-position drift bug
  blocking all ISLAST=1 metablocks is now resolved.
- ✅ **`brotli -q 1..8` decodes correctly on 100-byte all-'a' inputs**.
- ⚠️ `brotli -q 11` decodes on some inputs but fails on others with
  command-loop bugs.

## What remains

See TODO 174 for the dependency-ordered breakdown of remaining work.
Briefly:

- Multi-tree command loop bugs (q=11 multi-tree metablocks).
- Static dictionary with 121 transforms.
- Edge cases in skewed Huffman table reads.
- State-machine refactor for streaming support.

## Acceptance Criteria

- Decode all 11 brotli fixtures from upstream's test corpus.
- Decode every `.br` produced by `brotli -q 1` through `brotli -q 11`
  on the Linux kernel source.
- Differential test: 1000 random inputs through our decoder and
  `brotli -d` produce byte-identical output.
