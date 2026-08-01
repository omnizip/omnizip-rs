# ZSTD Huffman FSE-compressed weights — not yet ported

## Status

**Open.** Blocks `LITERALS_BLOCK_COMPRESSED` and
`LITERALS_BLOCK_TREELESS` literals decode.

## Affected code

`omnizip-zstd/src/literals/mod.rs` — `decode_compressed` returns
`ZstdError::Unsupported`.

## What RFC 8878 says

RFC 8878 §4.2.1 describes the Huffman table description format. The
header byte tells the decoder whether the weights are:

1. **Raw** — weights are listed verbatim, one byte per symbol.
2. **FSE-compressed** — weights are encoded using an FSE bitstream,
   read in reverse direction.

For FSE-compressed weights:

1. Read the FSE accuracy log from the header byte's low bits.
2. Read the FSE distribution table from the bitstream (RFC 8878 §4.1.1).
3. Initialise the FSE decoder state.
4. Decode weights one at a time until a sentinel (two consecutive
   zero weights) signals end-of-table.
5. Drop the last weight (it is a marker, not a real symbol weight).
6. Build the canonical Huffman table from the decoded weights.

## What the C reference does

The C reference (`lib/decompress/zstd_decompress_block.c`,
`ZSTD_decodeLiteralsBlock`) calls `HUF_readDTableX1` which
implements the full FSE-compressed-weights path.

## What the Rust port does

The Rust port's `HuffmanTable::from_weights` builds a canonical
Huffman table correctly from a weight array, but the table-reader
that would convert the FSE-compressed bitstream into a weight array
is not yet implemented. `literals::decode_compressed` returns
`ZstdError::Unsupported` when invoked.

## What the Ruby port does (bug)

The Ruby's `HuffmanTableReader.read_fse_compressed_weights` returns
an all-zero weight array as a "fallback" without reading any data.
See `../omnizip/BUGREPORT.01-huffman-fse-weights-stub.md`.

## Why the divergence exists

The FSE-compressed-weights reader depends on a working
`MODE_FSE` table reader for sequences (see
[compliance-zstd-fse-table.md](compliance-zstd-fse-table.md)),
because both use the same FSE-distribution-reading code path. Until
that lands, this path is gated.

## Impact

Any ZSTD frame using `LITERALS_BLOCK_COMPRESSED` (the common case for
non-trivial text) cannot decode. Raw and RLE literals decode
correctly.

## Reconciliation plan

1. Port the FSE distribution reader (RFC 8878 §4.1.1) — reads a
   normalised distribution from a bitstream given an accuracy log.
2. Use the FSE distribution reader to decode the Huffman weights.
3. Feed the weights into `HuffmanTable::from_weights`.
4. Wire the resulting `HuffmanTable` into `decode_compressed` and
   `decode_treeless`.

Estimated effort: 1 day, after the FSE table builder fix lands.
