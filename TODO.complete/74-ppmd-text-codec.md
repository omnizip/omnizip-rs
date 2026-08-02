# 74 — omnizip-ppmd: PPMd text compression codec

## Source
- Proposal: `../../limnifs/limnifs/docs/omnizip-ppmd-proposal.md`
- Academic: Cleary & Witten 1984 (PPM*), Shkarin DCC 2001 (PPMd)
- 7-Zip PPMd C source (LGPL-2.1) — black-box testing ONLY

## Why
PPMd beats Brotli q11 by 5-15% on natural-language text (enwik,
calgary corpus). For LimniFS document-heavy blobs (READMEs, source
code archives, HTML), PPMd at order 4-8 gives the best text ratio of
any available codec.

## Architecture

PPM (Prediction by Partial Matching) builds a context trie where each
node tracks symbol frequencies for the suffixes seen so far. When a
symbol is not found at the current order, an "escape" probability is
used and the model drops to a lower order. Shkarin's PPMd adds:
symbol exclusion, probability inheritance, and order truncation.

```
omnizip-ppmd/
  src/
    lib.rs              — public API + Codec trait
    context_tree.rs     — trie of suffix contexts (~500 LOC)
    probability.rs      — frequency-based probability model (~200 LOC)
    escape.rs           — escape mechanism (Shkarin method) (~200 LOC)
    range_coder.rs      — binary arithmetic coder (~100 LOC)
    model.rs            — Shkarin improvements (~500 LOC)
    codec.rs            — PpmdCodec struct
```

## Phased plan

| Phase | Scope | LOC | Gate |
|-------|-------|-----|------|
| 1 | Core PPM + escape (context_tree, probability, escape, range_coder) | ~1000 | order-4 round-trips; ~25% on enwik8 |
| 2 | Shkarin: symbol exclusion, prob inheritance, order truncation | ~500 | ≤ 20% on enwik8 |
| 3 | Optimization: sliding-window pruning | ~500 | ≤ 18% at order 8; ≥ 2 MB/s |
| CI | Differential vs 7-Zip black-box | ~200 | |
| **Total** | | **~2200** | |

## Acceptance criteria

1. `compress(enwik8, order=8)` ≤ 18 MB.
2. Beats Brotli q11 on ≥ 80% of Calgary Corpus text files.
3. Round-trip identity.
4. Determinism.
5. Memory: context tree stays within `mem_limit_mb`.
6. `#![forbid(unsafe_code)]`.

## Codec ID
`0x0C` (to be assigned by LimniFS).

## Shared infrastructure
The range coder could be shared with ZPAQ (both use binary arithmetic
coding). A potential `omnizip-arith` crate for shared entropy coding
primitives. This is optional — each codec can have its own coder.

## Key constraint
The context tree is the performance bottleneck. A naive trie has O(n·k)
memory for k orders. Shkarin's optimization uses a compact suffix-tree
representation. Phase 3 must address this or the codec will OOM on
large inputs.
