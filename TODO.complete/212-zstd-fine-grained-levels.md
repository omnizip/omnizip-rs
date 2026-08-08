# 212 — ZSTD Fine-Grained Levels (1–22)

- **Priority:** P2 (user control, enables level-specific tuning)
- **Crate:** `omnizip-zstd`
- **Depends on:** [211](211-zstd-strategy-dispatch.md)
- **Estimated effort:** 3 days

## Goal

Map every integer level 1–22 to distinct encoder parameters, matching
the C reference's preset table. Currently only 5 discrete levels
(1, 3, 6, 12, 22) are available via `ZstdLevel` enum.

## Background

The C reference maps each level to specific parameters:

| Level | Strategy | WindowLog | HashLog | ChainLog | SearchLog | MinMatch | TargetLength |
|-------|----------|-----------|---------|----------|-----------|----------|--------------|
| 1 | Fast | 19 | 18 | — | — | — | — |
| 2 | Fast | 20 | 19 | — | — | — | — |
| 3 | DFast | 21 | 20 | — | — | — | — |
| 5 | Greedy | 22 | 20 | 19 | 6 | 4 | 8 |
| 9 | Lazy | 22 | 21 | 20 | 6 | 4 | 16 |
| 13 | Lazy2 | 23 | 22 | 22 | 7 | 4 | 32 |
| 19 | BtOptimal | 24 | 23 | 23 | 9 | 4 | 88 |
| 22 | BtOptimal | 24 | 23 | 23 | 10 | 4 | 128 |

## Scope

1. **Level parameter table** (1 day): table of 22 preset parameter
   sets matching the C reference.

2. **API update** (1 day): accept `u8` level directly instead of
   `ZstdLevel` enum. Add `ZstdLevel::custom(n)` for arbitrary levels.

3. **CompressionLevel mapping** (1 day): map the workspace
   `CompressionLevel` (0–22) to ZSTD levels (1–22).

## Acceptance criteria

- [ ] Every level 1–22 produces distinct output
- [ ] Higher levels produce ≤ lower level output size
- [ ] Parameter table matches C reference within tolerance
- [ ] `zstd -d` accepts output at all levels
- [ ] API backward-compatible with existing `ZstdLevel` enum

## Implementation plan

### New module: `omnizip-zstd/src/encoder/level_presets.rs`

```rust
pub struct LevelParams {
    pub strategy: Strategy,
    pub window_log: u32,
    pub hash_log: u32,
    pub chain_log: u32,
    pub search_log: u32,
    pub min_match: u32,
    pub target_length: u32,
}

pub fn preset(level: u8) -> LevelParams {
    PRESETS[(level as usize - 1).min(21)]
}

const PRESETS: [LevelParams; 22] = [
    // Level 1
    LevelParams { strategy: Strategy::Fast, window_log: 19, hash_log: 18, ... },
    // Level 2
    LevelParams { strategy: Strategy::Fast, window_log: 20, hash_log: 19, ... },
    // ... levels 3-22
];
```

### Modified API

```rust
pub fn compress_at_level(plaintext: &[u8], level: u8) -> Result<Vec<u8>, ZstdError> {
    let params = level_presets::preset(level.clamp(1, 22));
    // ... compress with params
}
```

## Test plan

- Unit test: every level 1–22 produces valid output
- Unit test: level N+1 output ≤ level N output size
- Integration: `zstd -d` accepts output at all levels
- Benchmark: encode speed + ratio per level

## References

- C reference: `zstd/compress/clevels.h:ZSTD_defaultCLevel`
- RFC 8478 §3.1.1 (window descriptor)
