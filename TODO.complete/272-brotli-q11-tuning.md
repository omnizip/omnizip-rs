# 272 — Brotli Encoder Quality 11 (Q11) Tuning

- **Priority:** P2 (ratio: 1-3 pp additional at Q11)
- **Crate:** `omnizip-brotli`
- **Depends on:** [240](240-optimal-parser-expansion.md), [245](245-brotli-rep-codes-1-2-3-explicit.md)
- **Estimated effort:** 3 days

## Problem

Q11 currently uses the same `optimal_parse` as Q5/Q8. The C reference
at Q11 uses:
- Multi-pass optimal parsing (4+ iterations)
- Larger search depth (`max_chain` = 2,000+)
- NICE length up to 271 (MAX_COPY)
- Block-type switching for literal contexts (multiple Huffman tables)
- Context-modeled distance alphabet

We don't implement most of these. Result: Q11 is barely better than Q8.

## Design

### Q11-specific configuration

```rust
const Q11_CONFIG: ParserConfig = ParserConfig {
    max_chain: 1024,
    nice_match: 271,
    hash_log: 18,
    iterations: 4,  // iterative parser refinement
    enable_block_switch: true,   // requires decoder fix (TODO 244)
    enable_clustering: true,     // requires decoder fix (TODO 229)
};
```

### Iterative refinement (extended)

Currently 2 iterations at Q8+. Extend to 4 iterations at Q11:

1. Shannon cost from input
2. Shannon cost from iteration 1's literals
3. Huffman-aware cost from iteration 2's symbol stream
4. Refined Huffman from iteration 3

Each iteration is fast (DP is O(N * 35)). Total ~4× single-pass runtime.

### Block-type switching

Currently disabled. Once TODO 244 (decoder bugs) is fixed:
- Detect natural block boundaries (topical shifts in input)
- Emit NBLTYPESL > 1 with block-switch commands
- Each block has its own Huffman tree

Saves 5-15% on heterogeneous inputs (e.g., text+code+CSV mixed).

### Smart context clustering

Currently fixed `ctx >> 4` split. Data-dependent clustering exists
but produces decoder-rejected output (TODO 229). Once decoder is
fixed, enable cluster_contexts() output.

## Acceptance criteria

- [ ] Q11 uses `iterative_optimal_parse` with 4 iterations.
- [ ] CSV 100KB Q11 ratio improves to < 18% (currently 20.3%).
- [ ] English text Q11 ratio improves to < 0.5% (currently 0.6%).
- [ ] Block-type switching wired (pending decoder fix in TODO 244).
- [ ] Smart clustering wired (pending decoder fix in TODO 229).

## Why this matters

Q11 is the "max effort" level. Users expect it to be meaningfully
better than Q5/Q8. Currently it's roughly the same. Closing this gap
makes our Brotli competitive with the C reference at every quality.
