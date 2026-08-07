# 176: LZMA — Full Finish

## Priority: P1 (ratio + correctness)

## Status: documented — encoder bugs fixed (PR #171), remaining work is ratio + streaming.

## Context

The LZMA encoder had 8 interrelated bugs (fixed in PR #171) that caused
round-trip failures and xz-utils rejection. The encoder now produces
correct output at all levels (0-9) and all test fixtures round-trip.

Current ratio (post-fix, level 6 default):

| Fixture         | Orig  | LZMA  | Ratio |
|-----------------|-------|-------|-------|
| text_repeated   | 2000  | 104   | 5%    |
| binary_periodic | 10240 | 340   | 3%    |
| mixed           | 10000 | 884   | 9%    |

Reference `xz -6` achieves ~3-5% on the same fixtures. Our encoder is
competitive on periodic/binary data but loses on mixed content due to
the approximate optimal-parser prices.

## Remaining work

### A. LZMA2 multi-chunk state reuse (TODO 165)

**Problem**: `encode_lzma2_stream_with_options` creates a fresh
`Lzma1Encoder` per chunk (full probability-model reset). For inputs
larger than `MAX_CHUNK_UNCOMPRESSED` (2 MiB), each chunk starts with
un-adapted models, degrading ratio ~10-15% on the second+ chunk.

**Fix**: Carry the encoder's probability models + state across chunks.
The LZMA2 spec's reset-level field (bits 5-6 of the control byte)
already supports this:
- `0b00` = no reset (carry everything)
- `0b01` = reset state + reps
- `0b10` = reset state + reps + models
- `0b11` = full reset (current behavior)

**Implementation**:
1. Add `Lzma1Encoder::encode_chunk(input, eopm: bool) -> Vec<u8>` that
   does NOT flush/finish — just encodes and returns the raw stream.
2. The LZMA2 encoder creates ONE `Lzma1Encoder` and calls
   `encode_chunk` per chunk, choosing the reset level per chunk.
3. First chunk: reset_level=3 (full reset + props). Subsequent chunks:
   reset_level=0 (carry state).

**Files**: `encoder/lzma1.rs`, `encoder/lzma2.rs`

### B. Optimal-parser exact prices (TODO 106)

**Problem**: The optimal parser uses approximate prices (heuristic
length/distance slot costs). The C reference
(`lzma_encoder_optimum_normal.c`) computes exact state-conditioned
prices from the current probability models.

**Fix**: Replace the heuristic price functions in `prob_state.rs` with
actual range-coder-derived costs:
- `literal_price(state, byte)` → sum of `-log2(prob)` for each bit
- `match_price(state, distance, length)` → length + distance bit costs
- `rep0_price(state, length)` → rep flag + length bit costs

**Expected gain**: 1-3% ratio improvement on mixed content.

**Files**: `encoder/prob_state.rs`, `encoder/optimal.rs`

### C. BT4 match finder (TODO 108)

**Problem**: The encoder uses a hash-chain match finder. For level 9
(max compression), the C reference uses a binary-tree (BT4) match
finder that finds longer matches by maintaining a sorted binary tree
of positions.

**Fix**: Port the BT4 algorithm from `lz_encoder_mf.c`. The BT4 tree
allows O(log n) chain walking instead of O(n) for hash chains.

**Expected gain**: 2-5% ratio improvement at level 9 on large inputs.

**Files**: New `encoder/bt4_match_finder.rs`

### D. Streaming API (TODO 119)

**Problem**: No incremental encode/decode. All codecs operate on full
buffers.

**Fix**: Add `Lzma1Encoder::encode_chunk` (item A above) and a
`StreamingDecoder` that processes bytes as they arrive. The XZ
container already supports multi-block streams.

**Files**: `encoder/lzma1.rs`, `decoder/lzma1.rs`, new `streaming.rs`

## Acceptance criteria

- [x] All round-trip tests pass (147 LZMA tests)
- [x] xz-utils accepts all encoder output
- [ ] LZMA2 multi-chunk state reuse (ratio ≤ single-chunk on >2 MiB input)
- [ ] Optimal-parser exact prices (1-3% ratio improvement)
- [ ] BT4 match finder at level 9 (2-5% ratio improvement)
- [ ] Streaming encode/decode API
