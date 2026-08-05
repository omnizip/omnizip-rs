# TODO 146: Reusable-state pattern across all compressors

## Problem

`ZstdCompressor` and `PpmdCompressor` exist (TODOs 101, 118). Other
codecs still allocate per-call:

- LZMA: hash table + match finder per `compress`.
- FLAC: per-stream allocations.
- BZIP2: per-stream BWT + Huffman tables.
- DEFLATE / libdeflate: per-call LZ77 + Huffman tables.

For batch workloads with many small inputs, the per-call allocation
dominates.

## Proposed fix

Add a `*Compressor` reusable struct per codec that mirrors the
`ZstdCompressor` API:

```rust
pub struct LzmaCompressor { /* reusable match finder */ }
pub struct FlacCompressor { /* reusable LPC scratch */ }
pub struct Bzip2Compressor { /* reusable BWT scratch */ }
pub struct DeflateCompressor { /* reusable LZ77 + Huffman */ }
pub struct LibdeflateCompressor { /* same */ }
```

Each:
- Holds the codec's large allocations across calls.
- Resets adaptation state on each call (or exposes a `reset()`
  method).
- Produces byte-identical output to the one-shot API.

## Acceptance criteria

- [ ] Every codec with non-trivial allocation has a `*Compressor`.
- [ ] Each is verified byte-identical to its one-shot counterpart.
- [ ] Bench shows ≥ 3× speedup on 100-call batches of 1 KiB inputs.

## Priority

P1 — batch workloads are the main LimniFS use case.

## Dependencies

- Pattern established by `ZstdCompressor` (TODO 101) and
  `PpmdCompressor` (TODO 118).
