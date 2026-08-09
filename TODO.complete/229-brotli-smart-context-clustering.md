# 229 — Brotli Smart Context Clustering

- **Priority:** P2 (ratio win on text with diverse byte distributions)
- **Crate:** `omnizip-brotli`
- **Depends on:** [223](223-brotli-multi-context-trees.md) (NSYM>2 path
  must work — already fixed)
- **Estimated effort:** 2 days

## Goal

Expand from 2 literal context trees to 4+ with frequency-based context
clustering. The C reference assigns contexts to trees by clustering similar
byte-frequency distributions, achieving better specialization than our
current fixed 2-way split (contexts 0-31 → tree 0, 32-63 → tree 1).

## Background

With 64 contexts (LSB6 or UTF8 mode) and only 2 trees, each tree handles
32 contexts. Many of these contexts have similar byte distributions and
would benefit from sharing a tree, while others are very different and
should use separate trees.

The C reference uses a greedy clustering algorithm:
1. Compute per-context byte frequency histograms (64 × 256 array)
2. Start with each context in its own cluster (64 clusters)
3. Merge the two most similar clusters (by KL divergence or similar metric)
4. Repeat until the desired number of clusters (NTREES) is reached
5. Build a context map from contexts to cluster (tree) indices

## Current state

- NSYM > 2 path works correctly (fixed in 0.16.8)
- 4 trees tested but gave same/worse ratio because the fixed quarter-split
  (ctx/16) doesn't cluster similar contexts together
- Context map writing/reading verified for NSYM=4

## Plan

1. Implement context frequency histogram collection (64 × 256)
2. Implement greedy merging with KL divergence cost metric
3. Build context map from clustering result
4. Determine optimal NTREES (try 4, 8, 16 — pick best by estimated cost)
5. Inverse-MTF the context map for better compression

## Acceptance criteria

- [ ] Context clustering produces 4-8 trees from 64 contexts
- [ ] Each tree is specialized for a cluster of similar contexts
- [ ] Ratio improvement >= 1% on English text fixtures
- [ ] No regression on binary/uniform inputs
- [ ] Clustering cost is amortized over the metablock (one-time per block)
