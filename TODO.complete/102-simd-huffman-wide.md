# 102 — SIMD Huffman decode via `wide` crate (unblocks TODO 83)

**Priority:** Medium — unblocks TODO 83
**Source:** LimniFS proposal `omnizip-proposals/simd-huffman-wide.md`
**Status:** ⏳ Pending — implementation plan landed; PR TBD

## Problem

TODO 83 is **blocked** on `std::simd::simd_gather` stabilising on
stable Rust. Every `cargo` user is on stable; `std::simd` is
nightly-only. The TODO estimates a 1.5–3× throughput win on the
Huffman inner loop — the bottleneck for DEFLATE, Brotli, ZSTD, BZip2
decode.

## Proposed unblock

Use the [`wide`](https://crates.io/crates/wide) crate, which provides
portable SIMD types (`u8x16`, `u32x8`, etc.) on stable Rust today.
`wide` doesn't expose a `gather` primitive directly, but the Huffman
inner loop doesn't need one — it needs **batched table lookups**,
which we synthesise from `wide`'s shuffle + bit manipulation.

### The technique

Standard table-driven Huffman decode is sequential on the `consume`
step:

```text
loop:
    bits = peek(N)               # N = max code length
    sym = table[bits]            # one memory load
    consume(table[bits].len)     # update bit position
    output(sym)
```

The SIMD version processes 8–16 symbols per iteration by:

1. Peeking 8 × N bits at once (8 separate bit positions, precomputed
   from the code-length distribution).
2. Performing 8 table lookups in parallel using a `u32x8` index
   vector — emulated via `wide`'s primitives.
3. Writing 8 symbols to output via a single `u8x8` store.

The win is **not** from gather; it's from removing the sequential
`consume` dependency by batching the peek operations.

## Phased delivery

### Phase 1 — Scalar batching baseline (1 day)

Proves the batching win without SIMD. Decode N symbols per loop
iteration using scalar code, validating that batching alone is a win
on modern CPUs (it usually is — branch prediction is the limit).

### Phase 2 — `wide` batching in ZSTD Huffman (2 days)

- Add `wide = "0.7"` as optional dep, gated behind a `simd` feature.
- Implement `huffman::simd::decode_eight_symbols`.
- Differential test against scalar: byte-identical output required.

### Phase 3 — Roll out to other codecs (1 day each)

- `omnizip-deflate` (uses miniz_oxide — would need wrapping)
- `omnizip-brotli` (wraps brotli crate — same)
- `omnizip-bzip2` (our pure-Rust Huffman — directly applicable)

The wrappers may not benefit if the underlying crate is already
SIMD-optimised. ZSTD and BZip2 are the primary targets.

## Acceptance criteria

- [ ] `decode_eight_symbols` exists behind a `simd` feature flag in
      `omnizip-zstd`.
- [ ] On Enwik8 decompressed via ZSTD level 19, the SIMD path is
      ≥ 1.5× the scalar path's throughput.
- [ ] Output byte-identical to scalar (deterministic test).
- [ ] Default-feature build (no `simd`) is unchanged.
- [ ] No new `unsafe` code (`#![forbid(unsafe_code)]` preserved).

## Why `wide` instead of waiting for `std::simd`

| Path       | Status          | Portability             | Adds dep?                  |
|------------|-----------------|-------------------------|----------------------------|
| `std::simd`| nightly only    | x86 + ARM + WASM        | no                         |
| `wide`     | stable since 2021 | x86 SSE/AVX, ARM NEON | yes (small, no transitive) |
| `pulp`     | stable, ARM+x86 | wider                   | yes (heavier)              |

`wide` is the right choice today; migrate to `std::simd` when
`simd_gather` stabilises (Rust 1.85+ forecast).

## Effort estimate

4 days:
- 1 day: scalar batching baseline.
- 2 days: `wide` SIMD batching in ZSTD Huffman.
- 1 day: differential tests + benchmarks.

## Related

- omnizip-rs TODO 83.
- Kosolobov (2022), *Efficiency of ANS Entropy Encoders* — derives
  the batching bound theoretically.
- zlib-rs's SIMD Huffman (in C) — design reference.
