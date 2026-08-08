# 211 — ZSTD Strategy-Based Level Dispatch

- **Priority:** P2 (architectural improvement, enables level-specific tuning)
- **Crate:** `omnizip-zstd`
- **Depends on:** none
- **Estimated effort:** 1 week

## Goal

Implement strategy-based dispatch in the ZSTD encoder. The C
reference uses 5 strategies mapped to different compression levels.
Currently our encoder uses one strategy path for all levels.

## Background

ZSTD's C reference defines 5 compression strategies:

| Strategy | Levels | Algorithm |
|----------|--------|-----------|
| Fast | 1–2 | Single-pass hash, no chain |
| DFast | 3–4 | Double-pass hash (re-scan from position 0) |
| Greedy | 5–8 | Hash chain, take first match |
| Lazy | 9–15 | Hash chain, defer by 1–2 positions |
| Optimal (Zstd_opt) | 16–19 | DP backward references |
| BtOptimal (Zstd_btopt) | 20–22 | Binary tree + DP |

Our current encoder:
- Uses `compress_block_fast` for all levels (a single hash-chains
  approach with varying depth)
- Has `Strategy` enum but doesn't dispatch on it

## Scope

1. **Strategy trait** (2 days): define a `BlockCompressor` trait with
   `compress_block()` method. Each strategy implements it.

2. **Level → strategy mapping** (1 day): map `ZstdLevel` to the
   appropriate strategy.

3. **Strategy implementations** (4 days):
   - Fast (already exists in `compress_block_fast`)
   - Lazy (already exists in `compress_block_lazy`)
   - Lazy2 (already exists in `compress_block_lazy2`)
   - Greedy (new, from lazy2)
   - Optimal (TODO 210 prerequisite for cost model)

## Acceptance criteria

- [ ] All 5 strategies implemented and dispatch correctly
- [ ] Level 1 uses Fast, level 22 uses BtOptimal
- [ ] Ratio improvement ≥ 3% at levels 5–8 (Greedy vs current single-pass)
- [ ] `zstd -d` accepts output at all levels
- [ ] No encode speed regression > 20% at any level

## Implementation plan

### New module: `omnizip-zstd/src/encoder/strategy.rs`

```rust
pub trait BlockCompressor {
    fn compress_block(
        &mut self,
        input: &[u8],
        offset: usize,
        params: &CompressionParams,
        rep_offsets: &mut [u32; 3],
    ) -> Vec<Sequence>;
}

pub struct FastCompressor { hash_table: Vec<u32> }
pub struct DFastCompressor { hash_table: Vec<u32> }
pub struct GreedyCompressor { hash_table: Vec<u32>, chain_table: Vec<u32> }
pub struct LazyCompressor { hash_table: Vec<u32>, chain_table: Vec<u32> }
pub struct Lazy2Compressor { hash_table: Vec<u32>, chain_table: Vec<u32> }
pub struct OptimalCompressor { /* cost model */ }
```

### Level → strategy mapping

```rust
fn strategy_for_level(level: u8) -> Strategy {
    match level {
        1..=2 => Strategy::Fast,
        3..=4 => Strategy::DFast,
        5..=8 => Strategy::Greedy,
        9..=15 => Strategy::Lazy,
        16..=19 => Strategy::Lazy2,
        _ => Strategy::Optimal,
    }
}
```

### Integration with `block.rs`

Replace `compress_block_fast`/`compress_block_lazy`/`compress_block_lazy2`
calls with `strategy.compress_block(...)`.

## Test plan

- Unit test: each strategy produces valid sequences
- Unit test: level → strategy mapping is correct
- Integration: ratio improvement at levels 5–8
- Integration: `zstd -d` accepts output at all levels
- Benchmark: encode speed per level

## References

- RFC 8478 §3.2 (sequence encoding)
- C reference: `zstd/compress/zstd_compress_internal.h:ZSTD_strategy`
- Our encoder: `encoder/match_finder.rs` (has fast/lazy/lazy2)
- Our encoder: `encoder/cparams.rs:Strategy` enum (already defined)
