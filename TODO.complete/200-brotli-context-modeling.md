# 200 — Brotli Context Modeling (NTREES > 1)

- **Priority:** P1 (highest ratio win for text)
- **Crate:** `omnizip-brotli`
- **Depends on:** none (independent)
- **Estimated effort:** 1–2 weeks

## Goal

Implement literal context modeling with multiple Huffman trees per
metablock. This is the **#1 ratio driver for text** in Brotli: the
reference encoder at quality ≥ 4 uses up to 64 literal contexts,
each with its own Huffman tree.

## Background

RFC 7932 §9.6 + §10.1: each literal byte is encoded using a Huffman
tree selected by a **context ID** derived from the previous 1–2 bytes.
The context function depends on CONTEXT_MODE:

| Mode | Context function | Context count |
|------|-----------------|---------------|
| LSB6 | `(p1 & 0x3F)` | 64 |
| MSB6 | `(p1 >> 2)` | 64 |
| UTF8 | UTF-8-aware context | 64 |
| Signed | `((p1 + 1) >> 1) << 4 \| ((p2 + 1) >> 1)` | 256 |

The **context map** (RFC 7932 §9.6) maps each context to one of
NTREES_L Huffman trees. NTREES_L can be 1–256.

Current state: NTREES_L=1 (single tree for all literals), CONTEXT_MODE
hardcoded to LSB6. The encoder misses ~15% ratio on text.

## Scope

1. **Context computation** (2 days): implement LSB6, MSB6, UTF8, Signed
   context functions. Each returns a `u8` context ID from the previous
   1–2 output bytes.

2. **Context map construction** (3 days): build a context map that
   assigns contexts to trees. Start with NTREES_L=2 (binary split:
   ASCII vs non-ASCII). Future: cluster contexts by similarity.

3. **Per-tree Huffman** (2 days): partition literals by tree, build a
   separate `HuffmanLengths` per tree. Write all trees in the Huffman
   tree group (RFC 7932 §9.3).

4. **Context map encoding** (2 days): write the context map in the
   bitstream (RFC 7932 §9.6 format: RLE flag, inverse-MTF flag,
   per-entry Huffman-coded values).

5. **Literal encoding** (1 day): for each literal, compute its context,
   look up the tree via the context map, emit the Huffman code.

## Acceptance criteria

- [ ] NTREES_L ≥ 2 at quality ≥ 4
- [ ] All 4 context modes implemented (LSB6, MSB6, UTF8, Signed)
- [ ] Context map correctly encoded and decoded
- [ ] Round-trip correctness on all existing tests
- [ ] Ratio improvement ≥ 10% on text inputs vs NTREES_L=1
- [ ] `brotli -d` accepts our output
- [ ] No ratio regression on binary inputs

## Implementation plan

### New module: `omnizip-brotli/src/encoder/context.rs`

```rust
pub enum ContextMode { Lsb6, Msb6, Utf8, Signed }

pub trait ContextModel {
    fn context_id(&self, p1: u8, p2: u8) -> u8;
    fn num_contexts(&self) -> usize;
    fn mode(&self) -> ContextMode;
}
```

### Modified: `encode_huffman_chunk_into`

Replace `bw.write_bits(0, 2); // CONTEXT_MODE = LSB6` with actual mode
selection. Replace `bw.write_bits(0, 1); // NTREESL = 1` with
NTREES_L read via varlen. Write the literal context map.

### Modified: literal encoding loop

For each literal at position `i`, compute:
```rust
let p1 = if i > 0 { output[i - 1] } else { 0 };
let p2 = if i > 1 { output[i - 2] } else { 0 };
let ctx = model.context_id(p1, p2);
let tree_idx = context_map[ctx as usize];
let (code, len) = lit_codes_per_tree[tree_idx][literal];
bw.write_bits(code, len);
```

## Test plan

- Unit test: each context mode produces expected context IDs
- Unit test: context map encode/decode round-trips
- Integration: text inputs compress ≥ 10% better than NTREES_L=1
- Integration: `brotli -d` accepts output at all quality levels
- Differential: same input produces different output at different levels

## References

- RFC 7932 §9.3 (NTREES), §9.6 (context maps), §10.1 (context modes)
- Upstream: `brotli/c/enc/encode.c:ComputeContextModel`
- Our decoder: `decoder_full.rs:finish_metablock_decode` (already
  handles context maps + multi-tree groups)
