# 225 — ZSTD Optimal Parser (Btopt/Btultra)

- **Priority:** P3 (ratio win at L16+)
- **Crate:** `omnizip-zstd`
- **Depends on:** [224](224-zstd-bt-match-finder.md)
- **Estimated effort:** 3 days

## Goal

Implement the optimal parsing strategy for ZSTD levels 16+ (Btopt,
Btultra, Btultra2). The current encoder falls back to lazy2 parsing
for these levels, missing ratio opportunities.

## Background

Optimal parsing uses dynamic programming to find the globally optimal
sequence of literals and matches that minimizes total encoded size.
The C reference uses this at L16+:

1. Collect best match at each position (via BT match finder)
2. Build a cost model from literal/match frequency distributions
3. Backward DP: `cost[i]` = minimum bits to encode `input[i..n]`
4. Forward reconstruction: emit commands following the DP path

## Current state

- L13-L22 dispatch to `compress_block_lazy2` (look-ahead-2 parser)
- No optimal parsing exists
- The Btopt/Btultra/Btultra2 strategies in `CompressionParams` are
  recognized but not implemented (fall through to lazy2)

## Design

```rust
fn optimal_parse(
    src: &[u8],
    match_finder: &mut impl MatchFinder,
    rep_offsets: &mut [u32; 3],
    params: &CompressionParams,
) -> SeqStore
```

Cost model:
- Literal cost: Shannon entropy from byte frequency distribution
- Match cost: fixed overhead + offset/length extra bits
- Repcode cost: reduced offset encoding

## Acceptance criteria

- [ ] DP optimal parser implemented for L16+ strategies
- [ ] Cost model accurately predicts encoded size
- [ ] Round-trip tests pass at L16-L22
- [ ] Ratio improvement >= 3% on repetitive fixtures at L19+
- [ ] Encoding time <= 10× lazy2 (optimal parsing is inherently slower)
