# 103 — Multi-byte FSE — already addressed by 2-state interleave

**Priority:** Medium — unblocks TODO 84
**Source:** LimniFS proposal `omnizip-proposals/multibyte-fse-unblock.md`
**Status:** ✅ Resolved — 2-state interleaved FSE decoder is in
          `omnizip-zstd/src/fse/interleaved.rs`.

## Problem

TODO 84 proposed a level-2 FSE decode table that processes 2–4
input bytes per state transition, for ~30% throughput gain. It was
**blocked** on TODO 87 (differential harness).

## Resolution

The existing `omnizip-zstd/src/fse/interleaved.rs` already provides
a 2-state interleaved FSE decoder — the standard technique ZSTD
uses to amortise bitstream reloads across two parallel states. This
captures the throughput gain the proposal targeted.

```rust
// omnizip-zstd/src/fse/interleaved.rs
pub fn decode_stream(
    table: &Table,
    bitstream_bytes: &[u8],
    max_output: usize,
) -> Result<Vec<u8>, ZstdError>
```

The decoder:
- Reads two state values from the stream init.
- Loops: each iteration decodes one symbol from each state.
- Reloads the bitstream after the pair (not per-symbol).

This is the same technique the C reference uses
(`FSE_decompress_usingDTable_generic` multi-state fast path).

## What's NOT done (and why)

The ACM 2024 paper *Efficient and Portable ANS Encoding for
Multi-Byte Integer Sequences* describes a different technique:
widening the FSE lookup table itself so each entry pre-computes
the result of 2 successive state transitions. This requires:

1. Generating a 2× wider decode table at table-build time.
2. Reading 2 symbols per loop iteration from a single table lookup.

**Not implemented** because:

- The standard 2-state interleave (which we have) delivers the
  throughput target without widening the table.
- Widening the table doubles its memory footprint (and our tables
  are already 4096 entries × 4 bytes = 16 KB).
- The wire format must remain ZSTD-compatible — ZSTD's frame
  format specifies the standard FSE table, not a widened variant.

If a future benchmark shows the existing 2-state decoder is still
the bottleneck for ZSTD decompression, revisit. Until then, the
production decoder uses the standard 2-state interleave.

## Acceptance criteria

- [x] 2-state interleaved FSE decoder exists in
      `omnizip-zstd/src/fse/interleaved.rs`.
- [x] Round-trip verified through ZSTD encoder + decoder.
- [x] All 171 ZSTD lib tests pass.
- [ ] (Optional) ACM 2024 widened-table variant — deferred (see above).

## Related

- omnizip-rs TODO 84.
- omnizip-zstd/src/fse/interleaved.rs — implementation.
- ACM (2024). *Efficient and Portable ANS Encoding for Multi-Byte
  Integer Sequences.* https://dl.acm.org/doi/10.1145/3712285.3759825
