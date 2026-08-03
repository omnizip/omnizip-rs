# 81 — ZSTD dictionary trainer (FastCover algorithm)

**Priority:** High
**Source:** RESEARCH.md §3 (Learned compression with random access)

## Context

omnizip-zstd supports `compress_with_dict(input, level, dict)` but
has no way to TRAIN a dictionary from representative samples. Users
must shell out to `zstd --train` to produce one.

The FastCover algorithm (Facebook 2018) is the SOTA trainer:
1. Sample K bytes from each of N training documents.
2. For each offset in each sample, hash a K-byte window into one of
   2^log_k distinct buckets.
3. Pick the top-D highest-frequency segments as the dictionary.

FastCover is ~10x faster than the older COVER algorithm with similar
ratio.

## Existing skeleton

`omnizip-zstd/src/dict_trainer.rs` already exists with a basic
top-K-substrings trainer. This needs to be:
1. Replaced with the proper FastCover algorithm
2. Exposed via `dict_trainer::train(samples, target_size) -> Vec<u8>`
3. Tested against reference zstd dicts

## API

```rust
pub fn train_dict(
    samples: &[&[u8]],
    target_size: usize,    // typical: 110 KiB
    options: TrainOptions,
) -> Result<Vec<u8>, DictError>;

pub struct TrainOptions {
    pub k: usize,           // segment size (default 200)
    pub d: usize,           // dict size in segments (default 8)
    pub steps: usize,       // optimization steps (default 40)
    pub split_point: f64,   // training/validation split (default 0.75)
}
```

## Acceptance criteria

- [ ] FastCover algorithm implemented from scratch (no C FFI).
- [ ] Round-trip: train on samples → use as dict → compress test set.
- [ ] Ratio improvement ≥ 10% vs no-dict baseline on small text files.
- [ ] Performance: train 100 MB of samples in < 30s.
- [ ] Determinism: same samples + options → byte-identical dict.
- [ ] At least 5 unit tests covering edge cases (empty samples,
      single sample, target_size > total samples, etc.).

## Files

- `omnizip-zstd/src/dict_trainer.rs` — rewrite
- `omnizip-zstd/src/lib.rs` — export `train_dict`, `TrainOptions`
