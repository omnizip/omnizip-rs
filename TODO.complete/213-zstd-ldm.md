# 213 — ZSTD Long-Distance Matching (LDM)

- **Priority:** P3 (ultra-level ratio win, high memory cost)
- **Crate:** `omnizip-zstd`
- **Depends on:** [211](211-zstd-strategy-dispatch.md), [212](212-zstd-fine-grained-levels.md)
- **Estimated effort:** 1 week

## Goal

Implement long-distance matching (LDM) for levels ≥ 19. LDM uses a
sparse hash table to find matches at very large distances (beyond the
normal window), dramatically improving ratio on large files with
repeated blocks.

## Background

The C reference enables LDM at levels ≥ 19 (and for all levels when
`--long` is specified). LDM uses:
- A separate hash table with much larger coverage
- Coarser granularity (skip factor) to control memory
- Matches found by LDM are preferred over normal matches when they're
  longer

Typical ratio win: 5–20% on files > 1 MB with internal repetition.

## Scope

1. **LDM hash table** (3 days): separate sparse hash table with
   configurable window coverage.

2. **LDM match finder** (2 days): find long-distance matches alongside
   normal matches; prefer the longer match.

3. **Level gating** (1 day): enable LDM only at levels ≥ 19 or when
   explicitly requested.

4. **Memory management** (1 day): cap LDM hash table size based on
   window log.

## Acceptance criteria

- [ ] LDM enabled at levels ≥ 19
- [ ] Ratio improvement ≥ 5% on files > 1 MB with repetition
- [ ] Memory usage capped at `1 << window_log`
- [ ] `zstd -d` accepts output
- [ ] No ratio regression on small inputs (LDM disabled)

## Implementation plan

### New module: `omnizip-zstd/src/encoder/ldm.rs`

```rust
pub struct LdmHashTable {
    hash_log: u32,
    hash_table: Vec<u32>,
    chain_table: Vec<u32>,
    window_size: usize,
}

impl LdmHashTable {
    pub fn new(window_log: u32) -> Self { ... }

    /// Find a long-distance match at `pos`. Returns the longest match
    /// within the LDM window.
    pub fn find_match(&self, input: &[u8], pos: usize) -> Option<Lz77Match> { ... }

    /// Insert position into the LDM hash table.
    pub fn insert(&mut self, input: &[u8], pos: usize) { ... }
}
```

### Integration with strategy

In the Optimal/BtOptimal strategy, after finding normal matches, also
check LDM matches. If the LDM match is longer, use it instead.

## Test plan

- Unit test: LDM finds matches beyond normal window
- Integration: large file with repetition compresses better with LDM
- Integration: small file unchanged (LDM disabled)
- Benchmark: memory usage vs window_log

## References

- C reference: `zstd/compress/zstd_ldm.c`
- RFC 8478 §3.1.1.1.2 (window size and backward references)
