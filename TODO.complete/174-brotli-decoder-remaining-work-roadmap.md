# 174: Brotli — Decoder Remaining Work Roadmap

## Priority: P3

## Status: partial — scaffolding landed; multi-block-type still has bit-position drift bug.

## What landed (2026-08-06)

PRs #127, #130, #132. The full RFC 7932 decoder is now wired into
`decode_compressed_metablock` as a sibling path to the trivial fast
path. The full path lives in `omnizip-brotli/src/decoder_full.rs`.

- ✅ NPOSTFIX + NDIRECT distance codes (PR #127)
- ✅ UTF-8 + SIGNED context lookup tables + `ContextMode::context_id_2`
- ✅ Block-type machinery scaffolding (`BlockTypeState`,
  `read_block_type_trees`, `decode_switch`)
- ✅ Context map reader (`read_context_map` + inverse MTF)
- ✅ Tree-group reader (`read_tree_group`)
- ✅ Block-length reader using `kBlockLengthPrefixCode`
- ✅ `decode_compressed_metablock_full` wires everything together
- ✅ Top-level dispatch in `decode_compressed_metablock` (OCP)
- ✅ `brotli -q 1` interop test passes (trivial layout only)

## What remains

### Step 1: Block-type code reading — bit-position drift bug

**Bug**: For `brotli -q 11` output on small text inputs (e.g. 50 'a'
characters → 12-byte compressed stream), our decoder reads
`nbltypesc=204` per the varlen_uint8 arithmetic on bits 27-37. But
`brotli -q 11` should produce a single trivial-layout metablock for
such small inputs.

**What's been verified**:
- Frame header parse correct (window_bits=24)
- Metablock header parse correct (ISLAST=1, ISLASTEMPTY=0,
  MNIBBLES=4, MLEN=50, IS_UNCOMPRESSED=0, reserved=0)
- varlen_uint8 read matches upstream's `DecodeVarLenUint8` exactly
  (1 bit, then 3 bits, then up to 7 extra bits)
- Bit arithmetic for the 50-byte case: bits 27=1, 28-30=0b111=7,
  31-37=0b1001001=75, value=128+75=203, +1=204

The varlen_uint8 read produces 204 from the actual stream bytes.
That means upstream's decoder is doing something different that I'm
not seeing — possibly a different bit reader alignment, a different
metablock header offset, or a different `DecodeVarLenUint8` semantics
than RFC 7932 §9.3 says.

**Next step**: add a `println!` trace inside upstream's
`brotli-decompressor` to dump NBLTYPESL/C/D for the failing stream
and compare with our decoder.

### Steps 2-5

All scaffolding exists in `decoder_full.rs`. Once the bit-position
drift is fixed, these should work for metablocks that emit them.

## Acceptance Criteria

- Decode all 11 brotli fixtures from upstream's test corpus.
- Decode every `.br` produced by `brotli -q 1` through `brotli -q 11`
  on the Linux kernel source.
- Differential test: 1000 random inputs through our decoder and
  `brotli -d` produce byte-identical output.
