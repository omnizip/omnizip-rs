# 67 — ZSTD Treeless literals emission

**Status**: COMPLETED (0.9.3)

## What was done

Added Treeless (block_type=3) literals emission to the ZSTD encoder.
When consecutive compressed blocks in a frame produce identical Huffman
weight tables, the second (and subsequent) blocks emit the Treeless
literals header instead of re-sending the weights — saving ~60 bytes
per subsequent block.

## Implementation

- `huffman/encoder.rs`: new `encode_literals_with_weights` returns
  both the encoded bytes AND the weight wire bytes, plus an optional
  `treeless` flag that omits the weights from the output.
- `encoder/block.rs`: `last_huf_weights: Option<Vec<u8>>` threaded
  through the frame encoder. `encode_compressed_content` evaluates
  Treeless, Compressed (block_type=2 with new weights), and Raw, then
  picks the smallest.

## Test coverage

- All 133 existing ZSTD tests pass with Treeless enabled.
- Reference `zstd -d` accepts Treeless output (parity test green).
