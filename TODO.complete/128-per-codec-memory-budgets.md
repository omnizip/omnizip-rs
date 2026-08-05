# TODO 128: Per-codec memory budgets

## Problem

`Codec::compress` and `Codec::decompress` allocate as much memory
as they want. On memory-constrained devices (embedded LimniFS nodes)
a 1 GiB input can OOM.

## Proposed fix

Add an optional memory budget to the codec trait:

```rust
pub trait Codec {
    // ... existing methods ...

    /// Compress with a hard memory budget. Default implementation
    /// ignores the budget and falls through to `compress`.
    fn compress_with_budget(
        &self,
        plaintext: &[u8],
        level: CompressionLevel,
        budget: MemoryBudget,
    ) -> Result<Vec<u8>, OmnizipError> {
        let _ = budget;
        self.compress(plaintext, level)
    }
}

pub struct MemoryBudget {
    /// Max bytes the encoder may allocate.
    pub max_encoder_allocation: usize,
    /// Max scratch buffer for the decoder.
    pub max_decoder_scratch: usize,
}
```

Each codec that supports budgets overrides `compress_with_budget`:

- LZMA: clamp `dict_size` to fit budget.
- ZSTD: clamp `hash_log` and `chain_log`.
- Brotli: clamp quality (lower quality → less window).
- PPMd: clamp `memory_budget_bytes` (already supported in `compress_with_budget`).
- FLAC: clamp block size.

## Acceptance criteria

- [ ] `compress_with_budget` lands in `omnizip-codecs`.
- [ ] LZMA, ZSTD, PPMd, FLAC, Brotli all honour the budget.
- [ ] Tests confirm bounded allocation on 1 GiB inputs.
- [ ] Default fallback preserves current behaviour.

## Priority

P2 — important for embedded use cases, not on the LimniFS critical
path.
