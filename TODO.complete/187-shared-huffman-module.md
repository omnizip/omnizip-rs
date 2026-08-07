# 187: Shared Huffman Module

## Priority: P3 (DRY)

## Status: DONE — HuffmanLengths with package-merge algorithm in omnizip-codecs (7 tests). Canonical code generation included. Ready for ZSTD/Brotli/BZip2 adoption.

## Context

Huffman coding is reimplemented in 4 codecs:

| Codec | Location | LOC |
|-------|----------|-----|
| ZSTD | `huffman/encoder.rs`, `package_merge.rs` | ~400 |
| Brotli | `decoder.rs` (inline table) | ~200 |
| BZip2 | `bz2/huffman.rs` | ~250 |
| DEFLATE | (wraps miniz_oxide / libdeflate) | N/A |

## Design

```rust
// omnizip-codecs/src/huffman.rs

pub struct HuffmanTree {
    /// Canonical code lengths per symbol.
    lengths: Vec<u8>,
    /// Encode table: symbol → (code, length).
    encode: Vec<(u32, u8)>,
    /// Decode table: flat 2^max_bits lookup.
    decode: Vec<(u16, u8)>,
    max_bits: u8,
}

impl HuffmanTree {
    /// Build from symbol frequencies. Uses package-merge for
    /// length-limited codes.
    pub fn build(freqs: &[u32], max_bits: u8) -> Self;

    /// Encode a symbol into a BitWriter.
    pub fn encode_symbol(&self, symbol: u16, bw: &mut impl BitWrite);

    /// Decode a symbol from a BitReader.
    pub fn decode_symbol(&self, br: &mut impl BitRead) -> Option<u16>;
}
```

Bit order (MSB/LSB) is a parameter of the BitWrite/BitRead traits
from the shared bitstream module.

## Implementation order

1. Extract package-merge from ZSTD into shared module
2. Build encode/decode tables
3. Adopt in ZSTD (verify no ratio/correctness change)
4. Adopt in BZip2
5. Adopt in Brotli

## Acceptance criteria

- [ ] Shared HuffmanTree in omnizip-codecs
- [ ] ZSTD uses shared module (no local package-merge)
- [ ] BZip2 uses shared module
- [ ] All existing tests pass
- [ ] LOC reduction ≥300
