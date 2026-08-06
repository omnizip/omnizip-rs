# 174: Brotli Decoder — Remaining Work Roadmap

## Priority: P3

## Status: partial — ISLAST=1 metablock-header bug fixed; q=1..8 decode working for simple inputs.

## What landed (2026-08-06)

PRs #127, #130, #132, #133, #135, #136. The full RFC 7932 decoder
scaffolding is in place, with two OCP-compliant entry points sharing
a common tail.

- ✅ NPOSTFIX + NDIRECT distance codes (PR #127)
- ✅ UTF-8 + SIGNED context lookup tables + `ContextMode::context_id_2`
- ✅ Block-type machinery scaffolding (`BlockTypeState`,
  `read_block_type_trees`, `decode_switch`)
- ✅ Context map reader (`read_context_map` + inverse MTF)
- ✅ Tree-group reader (`read_tree_group`)
- ✅ Block-length reader using `kBlockLengthPrefixCode`
- ✅ `decode_compressed_metablock_full` + `_with_trees` + shared
  `finish_metablock_decode` tail.
- ✅ Top-level dispatch in `decode_compressed_metablock` (OCP)
- ✅ `brotli -q 1..8` interop test on simple inputs
- ✅ **`brotli -q 1..8` decodes correctly on 100-byte all-'a' inputs**
  (verified after the ISLAST=1 metablock-header fix in PR #135).

## Major bugs fixed

### Bit-position drift bug (PR #135)

`parse_metablock_header` was reading `IS_UNCOMPRESSED` + reserved bit
(2 bits) for ISLAST=1 metablocks, but upstream's
`METABLOCK_HEADER_UNCOMPRESSED` state only reads `IS_UNCOMPRESSED`
when `ISLAST=0` (and `is_metadata=0`). For ISLAST=1 metablocks the
body is always Huffman-coded — there's no `IS_UNCOMPRESSED` field.

Effect: dispatcher's NBLTYPES reads were 2 bits off for every ISLAST=1
metablock, producing absurd values like `nbltypesc=204` from real bits.

### Full decoder entry-point refactor (PR #136)

The full decoder had a bug: when dispatched from the trivial path's
NTREES > 1 branch, it re-read NPOSTFIX/NDIRECT/CONTEXT_MODE/NTREES
that the dispatcher had already consumed. Splits the full decoder
into two entry points sharing a `finish_metablock_decode` tail.

## What remains

### Step 1: Multi-tree command loop bugs (q=11 inputs)

`brotli -q 11` output on inputs > 50 bytes still fails with one of:
- "metablock overran mlen" — wrong command interpretation emits too
  many bytes.
- "invalid literal" — literal Huffman lookup returns a symbol outside
  the alphabet.
- "invalid code-length code lengths (space not consumed)" — Huffman
  table read produces an over-complete prefix code.

These point at multiple subtle bugs in:
1. **Context map interpretation**: the `context_map[context_id_2(p1, p2)]`
   indexing may be wrong, picking the wrong literal tree.
2. **Distance computation**: the static dictionary branch may be
   entered incorrectly, or distance code → distance value mapping
   may have a sign error for NPOSTFIX > 0.
3. **Huffman table reading for skewed distributions**: when one
   symbol dominates, the simple-form vs complex-form dispatch may
   pick the wrong path.

### Step 2: Static dictionary (RFC 7932 §10.3)

When `distance_code > max_distance`:
- Compute `word_id = distance_code - max_distance - 1`.
- Look up `kBrotliDictionaryOffsetsByLength[copy_len]` and
  `kBrotliDictionarySizeBitsByLength[copy_len]`.
- Extract the dictionary word and apply one of 121 transforms.

Files: new `dictionary.rs` submodule, `k_transforms` table,
`transform_dictionary_word` function.

LOC estimate: ~500 (mostly the constant dictionary data).

### Step 3: Skewed Huffman table edge cases

Inputs with very skewed distributions (e.g. all 'a' strings) hit
the NSYM=1 simple-form path. The current `read_simple_form` may not
handle all the bit-position edge cases correctly when combined with
the multi-tree dispatch.

## Acceptance Criteria

- Decode all 11 brotli fixtures from upstream's test corpus.
- Decode every `.br` produced by `brotli -q 1` through `brotli -q 11`
  on the Linux kernel source.
- Differential test: 1000 random inputs through our decoder and
  `brotli -d` produce byte-identical output.
