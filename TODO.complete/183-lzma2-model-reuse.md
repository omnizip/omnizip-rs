# 183: LZMA2 Full Probability Model Reuse

## Priority: P1 (ratio improvement on >2 MiB inputs)

## Status: partial — `encode_chunk_inplace` infrastructure exists

## Context

PR #173 added `base_pos` and `base_prev_byte` for multi-chunk
correctness. The encoder creates a fresh `Lzma1Encoder` per chunk
(reset_level=1 for subsequent chunks).

`encode_chunk_inplace(&mut self, ...)` was added to allow reusing a
single encoder across chunks (preserving probability models). However,
wiring it up with `reset_level=0` failed: the decoder's
`decode_continuation` path expects models to carry, but the encoder's
range coder state is byte-level and doesn't transfer across chunks
even with model carry. The mismatch produced incorrect output.

## Remaining work

1. **Understand the range-coder boundary**: The encoder flushes the
   range coder at the end of each chunk (5-byte padding). The decoder
   creates a fresh `RangeDecoder` per chunk. Models can carry, but
   the encoder's *state machine* (is_match context, etc.) must match
   what the decoder expects after a reset_level=0 chunk.

2. **Fix the encoder state after flush**: After flushing the range
   coder, the encoder's LZMA state (not models) may need a soft reset
   to match the decoder's expectations. Investigate whether the C
   reference resets anything on chunk boundaries with reset_level=0.

3. **Wire up**: Change LZMA2 encoder to use `encode_chunk_inplace`
   with `reset_level=0` for subsequent chunks once the above is fixed.

## Expected gain

~10-15% ratio improvement on inputs >2 MiB (models don't re-adapt from
scratch each chunk).

## Files

- `omnizip-lzma/src/encoder/lzma1.rs` — `encode_chunk_inplace` (done)
- `omnizip-lzma/src/encoder/lzma2.rs` — wire up + reset_level=0
- `omnizip-lzma/src/lzma2.rs` — decoder continuation path

## Acceptance criteria

- [ ] Multi-chunk inputs (>2 MiB) round-trip with reset_level=0
- [ ] Ratio on >2 MiB inputs improves ≥5% vs fresh-encoder-per-chunk
- [ ] All existing tests still pass
- [ ] `xz -d` accepts multi-chunk output
