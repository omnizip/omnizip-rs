# 177: ZSTD — Full Finish

## Priority: P1 (ratio + feature completeness)

## Status: documented — encoder works at zstd -1 level, remaining work is higher levels + features.

## Context

The ZSTD encoder produces valid frames accepted by `zstd -d`. Current
ratio matches `zstd -1` on most inputs but degrades on mixed content:

| Fixture         | Orig  | ZSTD  | Ratio |
|-----------------|-------|-------|-------|
| text_repeated   | 2000  | 43    | 2%    |
| binary_periodic | 10240 | 280   | 3%    |
| mixed           | 10000 | 1506  | 15%   |

The `mixed` fixture shows ZSTD losing to LZMA (9% vs 15%) because the
ZSTD encoder uses a single match-finder strategy for all levels.

## Remaining work

### A. Level-based match-finder differentiation (TODO 55, 107)

**Problem**: All compression levels (1-22) use the same hash-chain
match finder with the same parameters. The C reference (`zstd_compress.c`)
uses different strategies per level:

| Level | Strategy         | Chain | Nice | Window |
|-------|-----------------|-------|------|--------|
| 1-3   | Fast (no chain)  | 0     | 32   | 256KB  |
| 4-9   | Hash chain      | 4-256 | 64+  | 1-4MB  |
| 10-19 | Binary tree     | 1024+ | 273  | 8-64MB |
| 20-22 | Optimal parser  | 4096  | 273  | 64MB+  |

**Fix**: Map `CompressionLevel` to `(strategy, chain, nice, window)`
via a const table. The encoder dispatches to the right strategy.

**Expected gain**: 5-15% ratio improvement at levels 10+.

**Files**: `encoder/block.rs`, `encoder/match_finder.rs`, new
`encoder/strategy.rs`

### B. FSE sequence encoder completion (TODO 51)

**Problem**: The FSE encoder exists (`fse/encoder.rs`) but the
sequence-table path (MODE_FSE) may not produce optimal tables for all
input distributions.

**Fix**: Verify the FSE sequence encoder produces tables identical to
the C reference for the `huffman-compressed-larger.zst` fixture. Add
differential tests against `zstd --ultra -22`.

**Files**: `fse/encoder.rs`, `fse/from_stream.rs`

### C. Length-limited Huffman tree builder (TODO 57, 68)

**Problem**: The Huffman encoder uses a package-merge algorithm but may
not correctly enforce the ZSTD max-code-length constraint (11 bits for
literal/length, 10 for offsets).

**Fix**: Verify the package-merge implementation enforces the ZSTD
table-log limits. Add property tests that verify round-trip for all
symbol distributions.

**Files**: `huffman/package_merge.rs`, `huffman/encoder.rs`

### D. Dictionary support (TODO 76, 81)

**Problem**: No dictionary training or encoding support. The decoder
can decode dictionary-compressed frames but the encoder can't produce
them.

**Fix**: Port the ZSTD dictionary format:
1. `ZstdDictTrainer` — trains a dictionary from a sample corpus
2. `ZstdCodec::compress_with_dict(input, dict)` — uses a pre-trained dict

**Files**: New `dict_encoder.rs`, extend `dict_trainer.rs`

### E. Frame checksum + content size (TODO 70)

**Problem**: The encoder always writes the content size but may not
emit the frame checksum (XXHash64) correctly in all cases.

**Fix**: Verify the checksum flag is respected and the XXHash64 is
correct for all frame variants.

**Files**: `encoder/block.rs`, `frame/block.rs`

## Acceptance criteria

- [x] All round-trip tests pass
- [x] `zstd -d` accepts all encoder output
- [x] Ratio matches `zstd -1` on periodic/binary data
- [ ] Level differentiation (levels 10+ use BT match finder)
- [ ] FSE sequence tables match reference for all fixtures
- [ ] Dictionary encoding support
- [ ] Frame checksum correct for all variants
