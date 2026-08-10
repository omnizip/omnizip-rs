# 240 — Optimal Parser Expansion for Brotli

- **Status:** DONE (cost-aware DP with brotli-accurate distance cost
  model; considers all sub-match lengths via copy-code boundary
  sampling; handles length-changing dictionary transforms)
- **Priority:** P2 (moderate ratio win, high cost)
- **Crate:** `omnizip-brotli`
- **Depends on:** [238](238-multi-probe-hash-matching.md) (superseded)
- **Estimated effort:** 5 days

## Problem

The from_spec encoder uses lazy/lazy2 parsing (look-ahead 1-2
positions). The C reference uses optimal parsing via dynamic
programming, finding the globally cheapest command sequence.

Optimal parsing is implemented (`optimal_parse`) but capped at
inputs <= 64 KiB due to O(N²) complexity. For 1 MiB metablocks,
the cap means Q10-11 falls back to lazy2.

## Design

### Reduce DP complexity from O(N²) to O(N × max_matches)

Current: `cost[i] = min over all j > i of (cost[j] + match_cost(i, j))`
This is O(N²) because it considers all possible match endpoints.

Improved: `cost[i] = min over all match candidates at i of (cost[i + match_len] + match_cost)`
This is O(N × max_candidates_per_position) = O(N × max_chain).

### Implementation

1. Pre-compute match candidates at each position (via multi-probe)
2. DP backward: `cost[i] = min(literal_cost + cost[i+1], match_cost + cost[i+match_len])`
3. Forward reconstruction: walk the DP table to emit commands
4. Cost model: Shannon entropy for literals + fixed estimates for commands

### Memory

DP table: `Vec<u32>` of size N (4 bytes per position)
Match candidates: `Vec<Option<(dist, len)>>` of size N (8 bytes per position)
Total: 12N bytes. For 1 MiB metablock: 12 MB. Acceptable.

### Performance

O(N × max_chain) where max_chain=32 at Q5: O(32N) per metablock.
For 1 MiB metablock: 32M operations ≈ 0.3s. For 20 metablocks: 6s.
Acceptable for quality 10-11. For quality 5, keep lazy parsing.

## Acceptance criteria

- [ ] O(N × max_chain) optimal parser implemented
- [ ] Applied at Q10-11 for inputs up to 1 MiB metablocks
- [ ] CSV ratio improvement >= 10% at Q11
- [ ] Encoding time <= 5x lazy2 at Q11
