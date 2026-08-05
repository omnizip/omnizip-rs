# TODO 152: ZSTD per-se SIMD encoding

## Problem

LimniFS flags this as item #11: ZSTD's per-sequence encoding
(`FSE_encode_symbol`, `Huffman_encode_symbol`) is scalar today.
The 2× gap vs the C reference is partly because each sequence
is processed one at a time.

## Scope

Per-se SIMD targets:

1. **FSE state encoding**: compute the next-state table lookup for
   `lit_len`, `match_len`, `offset` symbols in parallel via SIMD.
2. **Huffman literal encoding**: encode 4-8 literals per cycle via
   `wide::u16x8` lookup tables.
3. **Sequence merging**: combine literal-run + match + distance into
   final FSE state in batch.

## Implementation plan

1. Profile `sequences::encode_section` to find the hottest path.
2. Vectorise the FSE state transition table read.
3. Bench against scalar; require ≥ 2× throughput.

## Acceptance criteria

- [ ] Per-se encode is SIMD-vectorised.
- [ ] Bench shows ≥ 2× throughput on text + binary inputs.
- [ ] Output byte-identical to scalar.

## Priority

P1 — LimniFS blocker #11.
