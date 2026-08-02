# 20 — ZSTD Huffman encoder

**Status**: ❌ Pending.

## Source

- `omnizip/lib/omnizip/algorithms/zstandard/huffman_encoder.rb` (336 LOC)

## Architecture

```rust
pub struct HuffmanEncoder {
    table: HuffmanTable,
}

impl HuffmanEncoder {
    /// Build an optimal Huffman table from `literals` (analyzes byte
    /// frequencies, computes weights, builds canonical codes).
    pub fn build_from_literals(literals: &[u8]) -> Self;

    /// Encode `src` into the wire format: 1 header byte + FSE-compressed
    /// weights + Huffman-coded data.
    pub fn encode(&self, src: &[u8]) -> Vec<u8>;
}
```

## Wire output

For each compressed-literals block:
1. Header byte (1 byte): iSize = number of bytes that follow for
   table + bitstream. If table fits in 127 bytes: iSize < 128 (FSE).
   Otherwise: iSize ≥ 128 (direct encoding).
2. Huffman weights (FSE-compressed or direct-packed).
3. Huffman-coded data (single-stream for <4096 bytes, 4-stream for
   larger).

## Files

- `omnizip-zstd/src/huffman/encoder.rs`
- Re-export from `huffman/mod.rs`

## Tests

- Round-trip: `HuffmanTable::from_weights(encoder.build_weights())`
  then decode → original.
- Determinism: encode same input 10× → identical output.

## Acceptance

- Used by task [21] (literals encoder).
