# ADR-0008: HashChainMatchFinder in omnizip-codecs

- **Status:** accepted
- **Date:** 2026-07-25
- **Deciders:** Ronald Tse, omnizip-rs maintainers

## Context

LZ77 match finding is the core hot path for every LZ-style codec:
LZMA, ZSTD, Brotli, LZ4_HC, libdeflate. Before this ADR, each
codec had its own implementation:

- `omnizip-lzma/src/encoder/match_finder.rs` — 400 LOC
- `omnizip-brotli/src/encoder/match_finder.rs` — 350 LOC
- `omnizip-zstd/src/encoder/match_finder.rs` — 500 LOC
- `omnizip-lz4/src/block.rs` (HC mode) — 250 LOC

Total: ~1,500 LOC of substantially similar code. Bugs in one (e.g.,
the recent O(N²) `match_length` cliff) had to be fixed in each.

## Decision

**Centralize hash-chain LZ77 match finding in
`omnizip-codecs/src/matchfinder.rs`** as `HashChainMatchFinder`.

The shared finder supports configuration via `HashChainConfig`:

```rust
pub struct HashChainConfig {
    pub dict_size: u32,         // max back-reference distance
    pub min_match: u32,         // typically 3 or 4
    pub max_chain_length: u32,  // depth vs. speed tradeoff
    pub nice_match: u32,        // early-exit threshold
    pub hash_log: u32,          // hash table size = 1 << hash_log
    pub max_match_length: u32,  // codec-specific cap (NEW)
}
```

Codecs instantiate with their own config:

- LZMA: `min_match=3, max_chain=256, max_match_length=273`
- Brotli: `min_match=4, max_chain=48, max_match_length=271`
- ZSTD: `min_match=3, max_chain=128, max_match_length=131072`
- LZ4_HC: `min_match=4, max_chain=64`

## Consequences

**Positive**:
- **DRY**: ~1,000 LOC removed across codecs.
- **Single bug-fix surface**: the match-length cap fix (TODO 110,
  2026-08-10) landed in one place and benefited all codecs.
- **Single perf-tuning surface**: SIMD acceleration, batch
  processing, etc. land once and benefit everyone.
- **Easier to audit**: one match-finder implementation to review,
  not four.
- **Foundation for future shared primitives** (bitstream, Huffman,
  checksum modules — TODOs 258, 249, 257).

**Negative**:
- **Lowest-common-denominator API**: features only one codec needs
  (e.g., LZMA's `prev`-array vs. ZSTD's binary tree) don't fit.
  Mitigated by per-codec wrappers (`new_lzma_match_finder`,
  `new_brotli_match_finder`).
- **`max_match_length` field added late** (TODO 244 follow-on): the
  first migration omitted it; adding it later required touching
  every codec. Lesson: when designing shared APIs, parameterize
  generously up front.
- **Codecs with non-hash-chain finders don't use this**: ZSTD L16+
  wants BT4 (TODO 257), LZMA L9+ wants BT4. These still have
  per-codec implementations.

**Neutral**:
- The shared finder is generic over `data: &[u8]` (no lifetime
  parameters in the trait); per-codec wrappers provide the typed
  interface.

## References

- [`omnizip-codecs/src/matchfinder.rs`](../../omnizip-codecs/src/matchfinder.rs)
- [TODO 114](../../TODO.complete/114-shared-match-finder.md) —
  original DRY spec.
- [TODO 233](../../TODO.complete/233-shared-match-finder-abstraction.md) —
  follow-on (deeper abstraction).
- [TODO 257](../../TODO.complete/257-lzma-bt4-match-finder.md) —
  BT4 for high-quality LZMA.
