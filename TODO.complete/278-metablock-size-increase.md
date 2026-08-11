# 278 — Metablock Size Increase (1 MiB → 16 MiB)

- **Priority:** P1 (perf — fewer chunk boundaries = less overhead)
- **Crate:** `omnizip-brotli`
- **Depends on:** none
- **Estimated effort:** 1 day

## Problem

Currently inputs > 1 MiB are split into ~1 MiB chunks, each emitted
as a separate metablock. For a 20 MiB CSV input that's 20 metablocks.

Per-metablock overhead:
- Huffman table emission (~50-200 bytes per metablock)
- Match-finder reset (rebuilds hash table per chunk)
- No cross-chunk match references (each chunk is independent)

The C reference uses up to 16 MiB metablocks. Fewer boundaries =
better ratio + faster (no per-chunk setup).

## Design

### MNIBBLES extension

RFC 7932 §9.2 allows MNIBBLES up to 6 (24-bit MLEN, max 16 MiB-1).
Currently we use MNIBBLES=0 (4 nibbles, max 64 KiB) or MNIBBLES=1
(5 nibbles, max 1 MiB).

Bump to MNIBBLES=2 (6 nibbles, max 16 MiB-1) for inputs > 1 MiB.

### Chunk size

```rust
// Before
let chunk_size = (1 << 20) - 1;  // 1 MiB - 1
// After
let chunk_size = (1 << 24) - 1;  // 16 MiB - 1
```

### Memory implications

Per metablock, the match finder allocates:
- Hash table: 1 << hash_log u32 entries (1 MiB at hash_log=18)
- Prev array: dict_size u32 entries (16 MiB at WBITS=24)

For 16 MiB metablock this is ~17 MiB per metablock. Acceptable on
modern hardware; embedded deployments may want smaller.

### Backward compatibility

Older decoders that only support MNIBBLES=0-1 will reject our
MNIBBLES=2 metablocks. RFC 7932 requires all decoders to support
MNIBBLES up to 6, so this is a non-issue for compliant decoders.

## Acceptance criteria

- [ ] Chunk size bumped to 16 MiB - 1.
- [ ] MNIBBLES encoding extended to support 6 nibbles.
- [ ] Round-trip verified on inputs up to 50 MiB.
- [ ] CSV-synthetic benchmark: 1-2s improvement from fewer chunks.
- [ ] No regression on small inputs (< 1 MiB).

## Why this matters

The chunking overhead is significant on multi-MiB workloads. Halving
the number of chunks (~10x reduction for 20 MiB input) gives both
ratio win (longer-range matches) and speed win (less per-chunk setup).
