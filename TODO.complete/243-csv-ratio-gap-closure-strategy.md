# 243 — CSV Ratio Gap Closure Strategy

- **Priority:** P0 (user's primary concern)
- **Crate:** `omnizip-brotli`
- **Depends on:** [238](238-multi-probe-hash-matching.md),
  [239](239-rep-code-optimization.md),
  [240](240-optimal-parser-expansion.md),
  [241](241-two-pass-backward-refs.md)
- **Estimated effort:** 2-3 weeks total

## Current state

| Metric | Our encoder | C reference | Gap |
|--------|------------|-------------|-----|
| CSV ratio | 20.8% | 3.6% | 5.8x |
| WAV speed | ~10s | 0.12s | 83x |
| Random speed | ~0.3s | 0.21s | 1.4x |

## Root cause analysis

The 5.8x CSV ratio gap comes from FOUR algorithmic differences:

### 1. Match finding (estimated 2-3x contribution)

Our encoder: single 4-byte hash probe per position.
C reference: multi-probe with backward reference collection.

Result: C reference finds 2-5x more matches on structured text.

Fix: TODO 238 (multi-probe) + TODO 241 (two-pass collection).

### 2. Parsing (estimated 1.5-2x contribution)

Our encoder: single-pass lazy (look-ahead 1-2 positions).
C reference: two-pass with optimal command assignment.

Result: C reference finds cheaper match combinations.

Fix: TODO 241 (two-pass) + TODO 240 (optimal parser).

### 3. Distance encoding (estimated 1.1-1.2x contribution)

Our encoder: checks rep0 only, uses explicit distance codes.
C reference: checks all rep codes, prefers rep matches.

Result: C reference saves 8-12 bits per repeated distance.

Fix: TODO 239 (rep code optimization).

### 4. Entropy coding (estimated 1.1-1.3x contribution)

Our encoder: 2-4 context trees, per-metablock Huffman.
C reference: 64+ context trees with clustering, block-split Huffman.

Result: C reference has better symbol distribution per tree.

Fix: TODO 229 (smart clustering) + TODO 242 (block split).

## Expected improvement from each fix

| Fix | Expected CSV ratio | Cumulative |
|-----|-------------------|------------|
| Current | 20.8% | — |
| + Multi-probe (238) | ~15% | 28% improvement |
| + Two-pass (241) | ~10% | 52% improvement |
| + Rep codes (239) | ~9% | 57% improvement |
| + Optimal parser (240) | ~7% | 66% improvement |
| + Block split (242) | ~6% | 71% improvement |
| + Smart clustering (229) | ~5% | 76% improvement |
| C reference | 3.6% | — |

## Implementation order

1. **TODO 239 (rep codes)** — 2 days, immediate ratio win
2. **TODO 238 (multi-probe)** — 3 days, biggest single win
3. **TODO 241 (two-pass)** — 5 days, fundamental algorithm change
4. **TODO 240 (optimal parser)** — 5 days, builds on 238+241
5. **TODO 242 (block split)** — 3 days, requires TODO 228
6. **TODO 229 (clustering)** — 2 days, requires decoder fix

Total: ~20 days for full gap closure.
