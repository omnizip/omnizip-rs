# 228 — Brotli Block Type Switching

- **Status:** DONE (infrastructure complete, feature disabled)
- **Priority:** P2
- **Crate:** `omnizip-brotli`
- **Implemented in:** 0.16.16 (timing fix), 0.16.8 (context map infrastructure)

## What was implemented

1. **Block switch timing fix** (0.16.16): The encoder now checks
   `lit_block_remaining == 0` BEFORE writing each literal (matching
   the decoder's check-block-length-then-read-literal order). The
   previous code checked AFTER, causing a one-literal offset.

2. **`write_block_type_trees`**: Writes the block-type code tree
   (NSYM=2, symbols 2/3) and block-length code tree (NSYM=1,
   symbol 12) with initial block length 128.

3. **Block switch emission**: In the encoding loop, emits the
   block-type symbol (1 bit) + block-length extra (5 bits) when
   the block boundary is reached.

4. **Frequency counting with block awareness**: The frequency
   counting loop tracks block types for correct tree assignment.

## Why it's disabled

`use_block_switch = false` because the full decoder path
(`decode_compressed_metablock_full`, triggered by NBLTYPES > 1)
produces "metablock overran mlen" when block switching is active.
The root cause is a wire-format interaction in the full decoder
that requires bit-level trace comparison against a reference
implementation to resolve.

The feature can be re-enabled by setting `use_block_switch = true`
once the decoder interaction is debugged.
