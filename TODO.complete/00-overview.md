# 00 — Overview

**Scope**: complete the LZMA + ZSTD ports — all decoders, all encoders,
all modes — so both crates can claim "fully ported" status.

## Current state (2026-08-01)

### omnizip-lzma
- ✅ LZMA-Alone (`.lzma`) decode — passes all `tests/fixtures/lzma/`
- ✅ XZ container (`.xz`) decode including BCJ-x86 filter
- ✅ Lzip (`.lz`) single-member decode; 7/8 multi-member fixtures fail
   (needs `member_size` trailer boundary detection)
- ❌ LZMA1 encoder
- ❌ LZMA2 chunk encoder
- ❌ XZ container encoder
- ❌ Lzip encoder

### omnizip-zstd
- ✅ Frame + block header parse
- ✅ Raw / RLE block decode
- ✅ Huffman decode (single-stream + 4-stream)
- ✅ Direct-encoded Huffman weights (iSize ≥ 128)
- ❌ FSE-compressed Huffman weights (iSize < 128) — blocks `huffman-compressed-larger.zst`
- ✅ Sequences decode with PREDEFINED + RLE modes
- ❌ MODE_FSE for LL/OF/ML sequence tables
- ❌ MODE_REPEAT for sequence tables
- ❌ XXHash32 checksum verification
- ❌ All ZSTD encoders

## Critical-path order

```
[01] ZSTD compressed literals decode ──── DONE (direct encoding only)
   ↓
[02] FSE-from-stream reader (shared) ───── unblocks [24] (FSE weights) + [25] (MODE_FSE)
   ↓                                             ↓
[24] FSE-compressed weights ─────────────  unblocks huffman-compressed-larger.zst
[25] MODE_FSE for sequences ─────────────  unblocks more fixtures
   ↓
[03] XXHash32 checksum ─────────────────── completes ZSTD decode

[04] Lzip multi-member ─────────────────── completes LZMA decode

[10..16] LZMA encoder (range → literal/length/distance → match finder → LZMA1 → LZMA2 → XZ → dispatch)
[20..25] ZSTD encoder (Huffman → literals → FSE → sequences → frame → dispatch)

[30..31] Determinism + differential encoder parity
[32] Yank 0.1.0, publish 0.1.1
```

## Architecture decisions

### Shared FSE table-from-stream reader

Tasks [24] and [25] both need the same primitive: read accuracy_log
from byte 0, decode the RLE-encoded normalized distribution, build an
FSE decode table. Will live in `omnizip-zstd/src/fse/from_stream.rs`
and be used by both:
- `huffman/weights.rs::read_fse_compressed_weights`
- `sequences.rs::get_table` for `MODE_FSE`

### Encoder structure mirrors decoder structure

Every encoder module sits next to its decoder sibling:
- `omnizip-lzma/src/coder/literal_encoder.rs` ↔ `literal_decoder.rs`
- `omnizip-lzma/src/range_coder/encoder.rs` ↔ `decoder.rs`
- `omnizip-zstd/src/huffman/encoder.rs` ↔ (existing decode code)
- etc.

No encoder reaches into decoder internals; both reach into shared
probability-model / tree-building primitives.

### Determinism strategy

Every encoder uses:
- Iteration over `BTreeMap` (not `HashMap`) where order matters.
- Deterministic match-finder traversal (no early-exit based on
  thread-local timing).
- Workspace buffers reused across calls (no per-call `Box::new`).

The differential parity test (task [31]) encodes the same input N=10
times and asserts byte-identical output.

## Invariants (from CLAUDE.md)

1. `#![forbid(unsafe_code)]` workspace-wide. SIMD via `std::simd`, never raw `unsafe`.
2. Determinism is non-negotiable.
3. No shims, no stubs. Replace `LevelUnavailable` etc. with real impls.
4. Spec-first. Wire-format changes update `docs/` first.
5. Rebase-merge all PRs.
