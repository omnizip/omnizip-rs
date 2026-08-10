# 242 — Block-Split Huffman Optimization

- **Priority:** P3 (moderate ratio win on diverse inputs)
- **Crate:** `omnizip-brotli`
- **Depends on:** [228](228-brotli-block-type-switching.md)
- **Estimated effort:** 3 days

## Problem

The from_spec encoder uses one Huffman table per metablock (or
per context tree). The C reference splits each metablock into
sub-blocks with different Huffman tables, optimizing each sub-block
for its local byte distribution.

For CSV data, the header row has different byte distribution than
data rows. Block splitting allows separate Huffman tables for
each, improving compression.

## Design

1. After parsing, analyze the literal distribution over positions
2. Find optimal split points using a cost-based criterion
3. Emit block-switch commands at split points (NBLTYPES > 1)
4. Build separate Huffman tables per block

This requires TODO 228 (block type switching) to be fully working.

## Acceptance criteria

- [ ] Block splitting implemented
- [ ] 2-4 blocks per metablock for diverse inputs
- [ ] CSV ratio improvement >= 2%
- [ ] Requires TODO 228 (block switch) to be enabled
