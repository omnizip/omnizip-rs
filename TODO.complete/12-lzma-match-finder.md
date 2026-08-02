# 12 — LZMA match finder

**Status**: ❌ Pending. Independent of [10]/[11] but blocks [13].

## Source

- `omnizip/lib/omnizip/algorithms/lzma/match_finder.rb` (233 LOC)
- `omnizip/lib/omnizip/algorithms/lzma/match_finder_config.rb`
- `omnizip/lib/omnizip/algorithms/lzma/match_finder_factory.rb`
- `omnizip/lib/omnizip/algorithms/lzma/xz_match_finder_adapter.rb`

## Architecture

Hash-chain match finder, mirroring XZ Utils `lz_encoder.c`:

```rust
pub struct MatchFinder {
    head: Vec<u32>,         // hash table: hash → most recent pos
    chain: Vec<u32>,        // prev[pos & mask] = previous pos with same hash
    data: &[u8],            // input
    cur: usize,             // current position
    config: MatchFinderConfig,
}

impl MatchFinder {
    pub fn new(data: &[u8], dict_size: u32) -> Self;
    pub fn reset(&mut self);
    pub fn next_match(&mut self) -> Option<Match>;
    pub fn current_position(&self) -> usize;
}

pub struct Match {
    pub distance: u32,
    pub length: u32,
}
```

The Ruby rebuilds the hash table on each call — the Rust port must
reuse allocations via `reset()` (per the porting-idioms note in
`CLAUDE.md`).

## Determinism

- Hash function: fixed table-based 4-byte → u32 hash. No `DefaultHasher`.
- Iteration order: sequential scan from `cur` through `chain` linked
  list. Deterministic by construction.

## Files

- `omnizip-lzma/src/encoder/match_finder.rs`
- `omnizip-lzma/src/encoder/match_finder_config.rs`

## Tests

- Hand-crafted input with known matches: assert exact match tuples.
- Random input: deterministic across 10 runs (same input → same match
  sequence).
- Stress: 100 MiB input doesn't panic or overflow.

## Acceptance

- Used by task [13] (LZMA1 encoder) without API changes.
