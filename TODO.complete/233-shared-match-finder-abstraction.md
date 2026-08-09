# 233 — Shared Match Finder Abstraction

- **Priority:** P3 (architecture — DRY across codecs)
- **Crate:** `omnizip-codecs` (shared module)
- **Depends on:** none
- **Estimated effort:** 3 days

## Goal

Extract the hash-chain match finder from both `omnizip-brotli` and
`omnizip-zstd` into a shared module in `omnizip-codecs`. Both codecs
currently have near-identical match finder implementations that have
diverged over time.

## Background

- `omnizip-codecs::HashChainMatchFinder` — used by Brotli
- `omnizip-zstd::encoder::match_finder::MatchState` — used by ZSTD

Both implement hash-table-based LZ77 matching with:
- 4-byte hash (multiply-shift)
- Hash chain for candidate walking
- Forward match extension via word-at-a-time comparison
- Backward match extension
- Repeat offset (repcode) checking

The implementations differ in:
- Hash table sizing (Brotli: per-call, ZSTD: per-frame)
- Chain depth control (Brotli: config struct, ZSTD: enable/disable)
- Distance capping (Brotli: MAX_BACKWARD_DISTANCE, ZSTD: BLOCK_MAX_SIZE)
- Output format (Brotli: Command, ZSTD: RawSequence)

## Plan

1. Define a generic `Lz77MatchFinder` trait in `omnizip-codecs`
2. Implement the trait for a concrete `HashChainMatcher` struct
3. Brotli and ZSTD wrap the shared implementation with codec-specific
   configuration and output formatting
4. Common algorithms (count_match, hash4, insert_range) live in one place

## Design (OCP)

```rust
pub trait Lz77MatchFinder {
    fn find_match(&mut self, pos: usize) -> Option<Lz77Match>;
    fn advance(&mut self);
    fn clear(&mut self);
}

pub struct HashChainMatcher {
    hash_table: Vec<u32>,
    chain: Vec<u32>,
    config: HashChainConfig,
}

impl Lz77MatchFinder for HashChainMatcher { ... }
```

Brotli and ZSTD create `HashChainMatcher` instances with codec-specific
configs and adapt the output.

## Acceptance criteria

- [ ] Shared match finder in `omnizip-codecs`
- [ ] Brotli uses shared implementation (no behavior change)
- [ ] ZSTD uses shared implementation (no behavior change)
- [ ] All existing tests pass (no regression)
- [ ] Future codec additions can reuse the shared finder
