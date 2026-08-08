# 215 — ZSTD Huffman Tree Optimization

- **Priority:** P3 (1% ratio win for literals, well-defined)
- **Crate:** `omnizip-zstd`
- **Depends on:** none
- **Estimated effort:** 2 days

## Goal

Improve literal Huffman tree selection. Currently uses a single
package-merge tree with max 11 bits. The C reference tries both
single-stream and FSE-compressed weight representations and picks the
smaller.

## Background

ZSTD literals can be encoded in 4 modes (RFC 8478 §3.1.1.3.1):

| Mode | Description | When to use |
|------|-------------|-------------|
| Raw (0) | No compression | Tiny inputs |
| RLE (1) | Single repeated byte | Uniform input |
| Compressed (2) | Huffman + optional FSE | Normal case |
| Treeless (3) | Reuse previous Huffman tree | Multi-block, same distribution |

Currently our encoder uses Compressed mode (2) for all blocks. The C
reference evaluates all 4 modes and picks the cheapest.

## Scope

1. **Mode evaluation** (1 day): for each block, compute the cost of
   each literal encoding mode and pick the cheapest.

2. **RLE detection** (0.5 days): detect single-byte-dominant inputs
   early.

3. **Treeless mode** (0.5 days): for multi-block frames, allow
   reusing the previous Huffman tree when it fits.

## Acceptance criteria

- [ ] Raw mode used for tiny blocks (< 64 bytes)
- [ ] RLE mode used for single-byte-dominant blocks
- [ ] Treeless mode used when previous tree fits
- [ ] Ratio improvement ≥ 0.5% overall
- [ ] `zstd -d` accepts output

## Implementation plan

### New function: `choose_literal_mode`

```rust
enum LiteralMode { Raw, Rle, Compressed, Treeless }

fn choose_literal_mode(
    block: &[u8],
    has_previous_tree: bool,
) -> LiteralMode {
    if block.len() < 64 {
        return LiteralMode::Raw;
    }
    // Check for single-byte dominance
    let mut freq = [0u32; 256];
    for &b in block { freq[b as usize] += 1; }
    let max_freq = *freq.iter().max().unwrap();
    if max_freq as usize == block.len() {
        return LiteralMode::Rle;
    }
    // Try treeless if previous tree exists
    if has_previous_tree {
        // Estimate cost of treeless vs compressed
        // ... (see TODO 210 for cost estimation)
    }
    LiteralMode::Compressed
}
```

## Test plan

- Unit test: tiny block uses Raw mode
- Unit test: single-byte block uses RLE mode
- Integration: multi-block frame uses Treeless where appropriate
- Integration: `zstd -d` accepts output

## References

- RFC 8478 §3.1.1.3.1 (literal block formats)
- C reference: `zstd/compress/zstd_compress.c:ZSTD_selectBlockType`
- Our encoder: `encoder/block.rs:write_literals_section`
