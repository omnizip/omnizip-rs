# 273 — LimniFS Workload Integration Tests

- **Priority:** P1 (real-world validation)
- **Crate:** workspace (`tests/limnifs_integration/`)
- **Depends on:** [247](247-real-world-test-corpora.md)
- **Estimated effort:** 2 days

## Problem

omnizip-rs is the primary compression layer for [LimniFS](https://github.com/limnifs/limnifs).
The LimniFS benchmark on `csv-synthetic` data is the headline metric
for the user. But:

- We don't have a copy of the `csv-synthetic` data.
- We don't test in the LimniFS configuration (multi-codec profiles:
  balanced/max-ratio/max-write).
- We don't validate determinism across LimniFS workloads.

Without these, "improves LimniFS perf by 5×" is a claim, not a
verified fact.

## Design

### LimniFS profile simulation

Each profile (balanced, max-ratio, max-write) uses a specific codec
mix. Replicate them in our harness:

```rust
fn balanced_profile() -> Ensemble {
    let mut e = Ensemble::new();
    e.register(/* Brotli Q5 for text */);
    e.register(/* ZSTD L9 for binary */);
    e.register(/* LZ4 L1 for max-write hot path */);
    e
}

fn max_ratio_profile() -> Ensemble {
    let mut e = Ensemble::new();
    e.register(/* Brotli Q11 for text */);
    e.register(/* LZMA L9 for binary */);
    e
}
```

### Realistic workload

Synthesize a LimniFS-like workload:
- 70% CSV files (column-aligned text)
- 15% binary blobs (object files, images)
- 10% JSON metadata
- 5% other (mixed)

Each file compressed with the ensemble's picker, then decompressed
to verify round-trip.

### Report

```
Profile     File-type   Codec   Compressed  Time    MB/s    Round-trip
balanced    csv         brotli  20.2%       0.5s    8.3     OK
balanced    binary      zstd    7.0%        0.1s    80.5    OK
max-ratio   csv         brotli  20.3%       5.2s    0.8     OK
max-ratio   binary      lzma    3.5%        8.1s    0.5     OK
```

## Acceptance criteria

- [ ] `tests/limnifs_integration/` with profile simulation.
- [ ] Synthesized LimniFS-like workload (10 MB CSV + 5 MB binary + ...).
- [ ] Report generation.
- [ ] Determinism check: same workload → same compressed bytes.
- [ ] Cross-machine check: Linux + macOS + Windows all produce
      identical output.

## Why this matters

Without LimniFS-specific tests, we don't know if our work actually
improves the user's experience. The synthetic tests we have are
useful but not representative.
