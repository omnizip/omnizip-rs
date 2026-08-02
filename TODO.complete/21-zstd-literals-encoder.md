# 21 — ZSTD literals encoder

**Status**: ❌ Pending. Depends on [20].

## Source

- `omnizip/lib/omnizip/algorithms/zstandard/literals_encoder.rb` (248 LOC)

## Architecture

Encodes the literals section for a compressed block:

```rust
pub enum LiteralsBlock<'a> {
    Raw(&'a [u8]),
    Rle(u8, usize),       // byte, count
    Compressed { literals: &'a [u8], huffman: HuffmanEncoder, single_stream: bool },
    Treeless { literals: &'a [u8], previous_huffman: &'a HuffmanTable },
}

pub fn encode_literals(block: LiteralsBlock) -> Vec<u8>;
```

## Per-block-type output

- **Raw**: 1-byte header (block_type=0, lhlCode=0) + raw bytes.
- **RLE**: 1-byte header (block_type=1) + 1 byte to repeat.
- **Compressed**: header byte + Huffman weights + 1 or 4 Huffman streams.
- **Treeless**: header byte (block_type=3) + 1 or 4 Huffman streams
  using the previous block's Huffman table.

## Decision heuristic

For each block of literals:
1. Try Raw (size = header + literals.len()).
2. Try RLE (size = header + 1, valid if all same byte).
3. Try Compressed (size = header + Huffman table + ceil(compressed_bits/8)).
4. Pick the smallest; emit the corresponding block_type.

This is the "smallest output wins" rule. Deterministic by construction.

## Files

- `omnizip-zstd/src/literals/encoder.rs`
- Re-export from `literals/mod.rs`

## Tests

- Round-trip: encode then `decode_literals_section` → identical bytes.
- Determinism: encode same literals 10× → identical output.

## Acceptance

- Used by task [24] (frame encoder).
