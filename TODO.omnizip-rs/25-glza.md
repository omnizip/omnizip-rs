# 25 — GLZA (grammar compression, research tier)

- **Priority:** P3 (research)
- **Depends on:** [01](01-codec-trait-registry.md)
- **Estimated effort:** unknown (research stage)
- **Crate:** `omnizip-glza` (future)

## Why

GLZA (Gregory L. Jackson 2017) is a grammar-based compressor: it builds a
context-free grammar representing the input, then encodes the grammar.
For highly-repetitive data (genomics, log files, versioned trees), GLZA's
ratio can exceed LZMA by 30–50%.

For LimniFS's delta-encoded epoch chain (where successive epochs share
most content), GLZA could be the ideal codec for the merged epoch
representation.

## Approach

The reference is at `grjav/GLZA` (GPL-3 → same license concern as ZPAQ).
The algorithm is documented in the IEEE Data Compression Conference 2017
paper.

**Status:** research only. No port until:
1. License question resolved (GPL-3 vs our MIT/Apache).
2. A clear LimniFS use case emerges (likely the epoch chain, post-item 02).
3. The algorithm's determinism is verified (GLZA's grammar construction
   has heuristics that may not be deterministic across runs).

## Open questions

- Is GLZA deterministic? If not, it violates LimniFS's content-addressing
  rule (same input must produce same output).
- What's the encode/decode speed? Grammar construction is expensive.
- Are there pure-Rust implementations to learn from? (None known as of 2026.)

**Defer indefinitely** unless a compelling LimniFS use case emerges.
