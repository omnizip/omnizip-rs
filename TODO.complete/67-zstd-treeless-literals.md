# 67 — ZSTD Treeless literals emission

## Gap

When consecutive compressed blocks in a frame share the same Huffman
table, the ZSTD spec lets the encoder emit them as **Treeless** blocks
(block_type = 3). This saves the ~60-byte weight table on each
subsequent block — significant for many-small-block frames.

Currently the encoder always emits Compressed blocks (block_type = 2)
with a fresh Huffman table per block, even when the table is unchanged.

## Implementation

1. **Track the last Huffman table** in `MatchState` (or a new
   `EncoderState`).
2. After building a Huffman table for the current block, compare its
   weights wire encoding against the previous block's.
3. If identical, emit block_type = 3 (Treeless) and omit the weights
   section.
4. The decoder already supports Treeless (see `literals::decode` →
   `is_repeat = true` path).

## Wire format difference

```
Compressed (type 2):
  header (3-5 bytes) | Huffman_Table_Description | jump_table | streams

Treeless (type 3):
  header (3-5 bytes) | jump_table | streams
  (reuses previous Huffman table from frame state)
```

## Test strategy

- Two identical literal distributions in a row → second block should
  be Treeless.
- Round-trip via own decoder and reference `zstd -d`.
