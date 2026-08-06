# 174: Brotli — Decoder Remaining Work Roadmap

## Priority: P3

## Status: pending

## Context

TODO 172 covers the full RFC 7932 decoder. As of v0.14.24 the
following pieces have landed:

- ✅ Frame header (RFC 7932 §9.1)
- ✅ Metablock header (§9.2)
- ✅ Uncompressed metablocks
- ✅ Trivial-layout Huffman-coded metablocks (single block type per
  category, NPOSTFIX=0/NDIRECT=0, single Huffman tree per category)
- ✅ Simple-form + complex-form Huffman tables (with iterated symbol
  16/17 repeat decoding)
- ✅ NPOSTFIX > 0 + NDIRECT > 0 distance codes (PR #127)
- ✅ UTF-8 + SIGNED context lookup tables (PR #127)
- ✅ `ContextMode::context_id_2(p1, p2)` (PR #127)

The decoder still rejects metablocks that use:

- Multiple block types per category (NBLTYPES > 1)
- Literal or distance context maps (NTREESL > 1 or NTREESD > 1)
- Static dictionary references (distance_code > max_distance)

These features are needed to decode brotli streams produced by
upstream's q ≥ 2 encoders on real-world inputs.

## Roadmap (in dependency order)

### Step 1: Block-type code reading (RFC 7932 §9.3)

For each category (literal, insert-copy, distance):
- `NBLTYPES` (already read via `DecodeVarLenUint8`).
- Block-type code: 1 + NBLTYPES Huffman codes, alphabet size
  `2 + NBLTYPES`.
- Initial block type + block length per category.
- Block-switch command decoding inside the command loop.

Files: `decoder.rs` — new `BlockTypeState` struct, `read_block_type_code`
function, integration into `decode_compressed_metablock`.

LOC estimate: ~400.

### Step 2: Huffman tree groups

Replace single `HuffmanTable` per category with a `Vec<HuffmanTable>`.
Index into the vector via:

- Literal trees: `context_map[context_id_2(p1, p2)]`
- Insert-copy trees: `block_type[1]` (no context map for commands)
- Distance trees: `dist_context_map[distance_context]`

Files: `decoder.rs` — new `HuffmanTreeGroup` struct, refactor of
`decode_compressed_metablock` to read NTREES instead of assuming 1.

LOC estimate: ~200.

### Step 3: Context-map reader (RFC 7932 §9.6)

`BrotliReadContextMap` port:
- NTREES via `DecodeVarLenUInt8`.
- MAXRLE = 16 × NTREES.
- Use the `kContextMapRleAlphabet` (2-symbol) Huffman code.
- Inverse MTF transform on the resulting tree-index array.
- Distance context map uses an additional XOR-with-1 step for the
  trivial distance context.

Files: `decoder.rs` — new `read_context_map` function.

LOC estimate: ~250.

### Step 4: Static dictionary (RFC 7932 §10.3 + Appendix B)

When `distance_code > max_distance`:
- Compute `word_id = distance_code - max_distance - 1`.
- Look up `kBrotliDictionaryOffsetsByLength[copy_len]` and
  `kBrotliDictionarySizeBitsByLength[copy_len]`.
- Extract the dictionary word and apply one of 121 transforms.

Files: new `dictionary.rs` submodule (or extend existing `dictionary.rs`),
`k_transforms` table, `transform_dictionary_word` function.

LOC estimate: ~500 (mostly the constant dictionary data).

### Step 5: Refactor command loop

Replace the current single-pass loop with a state machine that can:
- Suspend/resume across input chunks (for streaming).
- Handle block-switch commands mid-stream.
- Apply context lookups before each literal/dist read.

This is the largest piece; mirrors upstream's `ProcessCommandsInternal`.

LOC estimate: ~600.

## Acceptance Criteria

- Decode all 11 brotli fixtures in upstream's test corpus.
- Decode every `.br` produced by `brotli -q 1` through `brotli -q 11`
  on the Linux kernel source.
- Differential test: 1000 random inputs through our decoder and
  `brotli -d` produce byte-identical output.

## Why this is a substantial effort

The decoder state machine is ~3.5K LOC in upstream Rust. Even a
faithful port requires careful attention to:
- Bit-reader suspend/resume across input boundaries.
- Ring-buffer wraparound for back-references near the start of stream.
- Block-type code Huffman tree construction (separate from main trees).
- Context-map RLE + inverse MTF decoding (its own sub-state-machine).

Each step above is independently testable against `brotli -d` for
regression, so progress can land incrementally.
