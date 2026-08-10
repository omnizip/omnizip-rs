# 260 — Codec Parallel Batch API

- **Priority:** P2 (throughput — LimniFS multi-file workloads)
- **Crate:** `omnizip-codecs`
- **Depends on:** [251](251-codec-streaming-api.md)
- **Estimated effort:** 2 days

## Problem

LimniFS and similar batch workloads compress many independent files
in a tight loop. Current API:

```rust
for file in files {
    let compressed = codec.compress(&file.data, level)?;
    store(&file.id, &compressed);
}
```

This is sequential. Each file waits for the previous to finish. On
multi-core machines, 8 cores do the work of 1.

## Design

### Parallel batch trait

```rust
pub trait ParallelBatch {
    /// Compress many inputs in parallel using rayon.
    /// Returns results in input order.
    ///
    /// Each input is compressed independently — no shared state
    /// across inputs (determinism guarantee).
    fn compress_batch(
        &self,
        inputs: &[&[u8]],
        level: CompressionLevel,
    ) -> Result<Vec<Vec<u8>>, OmnizipError>;

    /// Decompress many inputs in parallel.
    fn decompress_batch(
        &self,
        inputs: &[&[u8]],
        expected_lens: &[u32],
    ) -> Result<Vec<Vec<u8>>, OmnizipError>;
}
```

### Default implementation

```rust
impl<T: Codec> ParallelBatch for T {
    fn compress_batch(
        &self,
        inputs: &[&[u8]],
        level: CompressionLevel,
    ) -> Result<Vec<Vec<u8>>, OmnizipError> {
        inputs
            .par_iter()  // rayon
            .map(|input| self.compress(input, level))
            .collect()
    }
}
```

### Determinism guarantee

Each input is compressed in its own task. No shared mutable state.
The same input + level produces byte-identical output regardless of:

- Number of inputs in the batch
- Order of inputs in the batch
- Thread scheduling

Verified by a determinism test that runs the same input through
batches of varying sizes and orders.

### Backpressure

Use rayon's built-in work-stealing. The caller can limit
parallelism via `rayon::ThreadPoolBuilder`:

```rust
let pool = rayon::ThreadPoolBuilder::new()
    .num_threads(4)
    .build()?;
pool.install(|| codec.compress_batch(&inputs, level))?;
```

## Acceptance criteria

- [ ] `ParallelBatch` trait with default impl in omnizip-codecs.
- [ ] All codecs get parallel batch via blanket impl.
- [ ] Determinism test: same input, different batch sizes/orders,
      produces identical output.
- [ ] Speedup on 8-core machine: 6-8× on 100-file batches.
- [ ] Example: `omnizip-bench/examples/parallel_batch.rs`.

## Why this matters

Multi-core machines are the norm. Sequential APIs waste 7/8 of
available compute. LimniFS batch-ingest workloads (e.g., importing
a directory) would see 6-8× throughput improvement.
