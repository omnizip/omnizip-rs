# 18 — PPMd7 / PPMd8

- **Priority:** P2 (excellent for text; niche use)
- **Depends on:** [01](01-codec-trait-registry.md), [02](02-cross-language-differential-harness.md)
- **Estimated effort:** 3 weeks
- **Crate:** `omnizip-ppmd`

## Goal

Port PPMd7 (Dmitry Shkarin 2001) and PPMd8 (7-Zip's variant). PPMd uses
predictive context modeling — its text ratio often beats LZMA. Used in
`.7z` archives for text-heavy content.

## Ruby → Rust module map (1,463 LOC)

### PPMd7 (807 LOC)

| Ruby source | Rust module | LOC |
|---|---|---:|
| `ppmd7/constants.rb` | `ppmd7/constants.rs` | ~100 |
| `ppmd7/model.rs` | `ppmd7/model.rs` | ~300 |
| `ppmd7/context_table.rb` | `ppmd7/context.rs` | ~200 |
| `ppmd7/range_coder.rb` | `ppmd7/range_coder.rs` | ~200 |
| `ppmd7/encoder.rb` | `ppmd7/encoder.rs` | ~150 |
| `ppmd7/decoder.rb` | `ppmd7/decoder.rs` | ~150 |

### PPMd8 (656 LOC)

Similar structure, slightly different model. Port after PPMd7.

## Acceptance

- **Differential gate:** Ruby and Rust produce byte-identical output at
  every model order and memory setting on every corpus fixture.
- **C reference gate:** Rust output decompresses through 7-Zip's PPMd
  decoder.
- Ratio within 5% of 7-Zip's PPMd on text-heavy fixtures (enwik, dickens).
- Decode throughput ≥ 10 MB/s; encode throughput ≥ 5 MB/s.
- Clippy clean, no `unsafe`, deterministic.

## Implementation notes

- PPMd is memory-heavy: the model allocates a configurable context pool
  (typically 64 MB – 256 MB). Make this configurable via `PpmdOptions`.
- The model update is the hot path — incremental probability updates
  per symbol. Port carefully; the Ruby's update is the reference.
- PPMd7 and PPMd8 differ in subtle ways. Implement PPMd7 first; PPMd8 is
  backwards-compatible at the format level.
