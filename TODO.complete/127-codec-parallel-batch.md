# TODO 127: Codec concurrency: parallel batch compress

## Problem

LimniFS batch workloads compress thousands of files in a tight loop.
The current `Codec` trait is synchronous; a multi-GB batch on a
multi-core CPU is single-threaded.

## Proposed fix

Add an extension trait `ParallelCodec` in `omnizip-codecs`:

```rust
pub trait ParallelCodec: Codec {
    fn compress_parallel(
        &self,
        inputs: &[&[u8]],
        level: CompressionLevel,
        threads: usize,
    ) -> Result<Vec<Vec<u8>>, OmnizipError>;
}
```

Default implementation uses `rayon` (behind a `parallel` cargo
feature on `omnizip-codecs`). Per-codec overrides can use
codec-specific parallelism (e.g., ZSTD's frame-level parallelism).

## Determinism

Parallel execution must preserve input ordering in the output
vectors. Within each call, output `[i]` corresponds to input `[i]`.
Each individual `compress` is still deterministic; parallelism is
only at the call boundary.

## Acceptance criteria

- [ ] `ParallelCodec` trait lands in `omnizip-codecs`.
- [ ] At least LZMA, ZSTD, DEFLATE, LZ4 implement it.
- [ ] Bench shows ≥ 4× speedup on 8-core machines for 100-file
  batches.
- [ ] Differential parity: same output as serial calls.

## Priority

P2 — important for LimniFS throughput, but not a correctness issue.
