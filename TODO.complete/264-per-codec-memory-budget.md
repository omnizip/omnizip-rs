# 264 — Per-Codec Memory Budget API

- **Priority:** P2 (embedded / constrained deployments)
- **Crate:** workspace
- **Depends on:** [251](251-codec-streaming-api.md) (streaming memory_usage)
- **Estimated effort:** 2 days

## Problem

Codecs allocate varying amounts of memory depending on input size,
level, and internal state. Callers have no way to ask "how much
memory will this take?" before starting.

For LimniFS (in-memory FS) and embedded deployments, this matters:
- Compressing a 1 GiB file at LZMA level 9 might allocate 64 MiB
  of match-finder state.
- ZSTD at L19 allocates ~8 MiB window.
- Brotli at Q11 allocates dictionary + chain.

## Design

### Memory budget estimation

```rust
/// Estimated peak memory usage for a codec at a given level on
/// input of size `input_len`.
pub trait MemoryBudget {
    fn estimated_peak_memory(
        &self,
        input_len: usize,
        level: CompressionLevel,
    ) -> usize;
}
```

Each codec overrides:

```rust
impl MemoryBudget for LzmaCodec {
    fn estimated_peak_memory(&self, input_len: usize, level: CompressionLevel) -> usize {
        let window = match level.as_u8() {
            0..=2 => 1 << 16,
            3..=5 => 1 << 18,
            6..=9 => 1 << 24,
            _ => 1 << 26,
        };
        let chain = 256 * 4;  // max_chain * sizeof(u32)
        window + chain + input_len
    }
}
```

### Caller pattern

```rust
let codec = registry.get(CodecId::LZMA)?;
let needed = codec.estimated_peak_memory(file_size, CompressionLevel::new(9));
if needed > budget {
    return Err(OutOfBudget { needed, budget });
}
```

## Acceptance criteria

- [ ] `MemoryBudget` trait in omnizip-codecs.
- [ ] All 15 codecs impl with accurate estimates.
- [ ] Validated by allocation-tracking tests (use `dhat` or similar).
- [ ] Documentation shows expected memory per codec per level.

## Why this matters

Without memory budgets, callers must guess. Embedded systems run
out of memory mid-compress. Servers kill processes that grow too
big. Budgets let callers choose codecs adaptively based on what
they can afford.
