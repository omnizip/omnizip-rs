# 102 — SIMD Huffman decode via `wide` crate (unblocks TODO 83)

**Priority:** Medium — unblocks TODO 83
**Source:** LimniFS proposal `omnizip-proposals/simd-huffman-wide.md`
**Status:** 🔄 Phase 1 landed (scalar batching); Phase 2–3 REJECTED after analysis.

## Problem

TODO 83 is **blocked** on `std::simd::simd_gather` stabilising on
stable Rust. Every `cargo` user is on stable; `std::simd` is
nightly-only. The TODO estimates a 1.5–3× throughput win on the
Huffman inner loop — the bottleneck for DEFLATE, Brotli, ZSTD, BZip2
decode.

## Phase 1 — scalar batching (LANDED)

`HuffmanDecoder::decode_into` unrolls the inner loop in groups of
8 symbols. The bitstream reload happens once per group instead of
per-symbol. The 8 dependent decodes get scheduled back-to-back by
the compiler, eliminating reload interference.

Measured improvement: 5–15% on ZSTD Huffman-heavy payloads (depends
on code-length distribution).

This is the highest-ROI change that doesn't require `unsafe` and is
already shipped.

## Phase 2 — `wide` crate SIMD (REJECTED)

The original plan was to use [`wide`](https://crates.io/crates/wide)
(`u32x8` SIMD on stable Rust) to parallelise the 8 table lookups.
**Investigation result:** `wide` does not expose a gather primitive.

The Huffman inner loop's bottleneck is:
```text
sym = table[bits]          # one random memory load
consume(table[bits].len)   # sequential dependency on sym
```

The 8 batched lookups each load from a different table index. True
SIMD requires either:

- **Gather**: `table[u32x8]` — loads 8 elements at 8 indices in one
  SIMD instruction. Available in `std::simd` (nightly only) and in
  x86 AVX2 intrinsics (`unsafe`). **Not in `wide`.**
- **Shuffle**: limited to tables that fit in a SIMD register
  (16 entries max for u8x16). Huffman tables are 4096 entries —
  way too big.
- **Stream interleaving**: encoder-coordinated interleave of 8
  independent streams. Requires changing the wire format — not
  viable for ZSTD/DEFLATE compatibility.

Without gather, the 8 lookups must happen serially. The scalar
batching in Phase 1 already exposes all the available ILP; the
compiler auto-vectorises the surrounding arithmetic.

**Decision:** Phase 2 is **REJECTED**. The `wide` crate cannot
deliver the proposed speedup without violating
`#![forbid(unsafe_code)]`.

## Phase 3 — Roll out to other codecs (REJECTED)

Phase 3 depends on Phase 2; closed along with it.

## Path forward (when feasible)

When `std::simd::simd_gather` stabilises on stable Rust (forecast:
Rust 1.85+), revisit. At that point we can implement true SIMD
gather behind a `simd` feature flag without `unsafe`.

Until then, the production decoder uses Phase 1 scalar batching.
The `wide` crate remains a viable dep for non-Huffman SIMD work
(CRC, hash, vector arithmetic).

## Acceptance criteria

- [x] `decode_into` 8-symbol unroll (Phase 1) — landed in
      `omnizip-zstd/src/huffman/mod.rs`.
- [x] Differential test: byte-identical to per-symbol loop.
- [x] Phase 2/3 rejected with documented rationale.
- [ ] SIMD gather via `std::simd` — deferred until stable.

## Related

- omnizip-rs TODO 83.
- omnizip-rs TODO 101 (ZSTD hash_log cap — the other ZSTD perf win).
- Kosolobov (2022), *Efficiency of ANS Entropy Encoders* — derives
  the batching bound theoretically.
- zlib-rs's SIMD Huffman (in C with `unsafe`) — design reference
  for what we'd do once `std::simd` is stable.
