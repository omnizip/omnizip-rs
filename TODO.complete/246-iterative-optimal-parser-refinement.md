# 246 — Iterative Optimal Parser Refinement

- **Priority:** P2 (additional 2-5% ratio win)
- **Crate:** `omnizip-brotli`
- **Depends on:** [240](240-optimal-parser-expansion.md) ✅
- **Estimated effort:** 3 days

## Problem

The current `optimal_parse` (TODO 240) uses Shannon entropy as the
literal cost model and a flat `7 + dist_cost(dist)` for matches.
This is a good first-order approximation but diverges from the
actual encoding cost in two ways:

1. **Literal Huffman code lengths** depend on the per-context byte
   frequency distribution. Common bytes (e.g., `,` in CSV) get
   2-3 bit codes; rare bytes get 10+ bit codes. Shannon entropy
   captures the average but not the per-byte variance.

2. **Command Huffman code lengths** depend on the command mix.
   Common commands (insert 1, copy 4 at short distance) get short
   codes (~4 bits); rare commands get long codes (~10 bits).

The DP uses fixed estimates, so it may pick a "cheaper" command
sequence that's actually more expensive once Huffman tables are
built for it.

## Design

### Two-pass refinement

1. **Pass 1**: Run `optimal_parse` with current cost model. Build
   the symbol stream. Construct Huffman tables for literals,
   commands, distances.
2. **Pass 2**: Replace the cost model with the actual Huffman code
   lengths from Pass 1's tables. Re-run `optimal_parse`. The DP
   now sees accurate per-symbol costs.
3. **Convergence**: Typically 2 iterations suffice. Cap at 3 to
   bound runtime.

### Cost model interface

```rust
trait CostModel {
    fn literal_cost(&self, prev_byte: u8, byte: u8) -> f32;
    fn match_cost(&self, dist: u32, copy_len: u32, insert_len: u32) -> f32;
}

struct ShannonCostModel { /* current implementation */ }
struct HuffmanCostModel {
    lit_codes: Vec<u8>,    // per-context literal code lengths
    cmd_codes: Vec<u8>,    // command code lengths
    dist_codes: Vec<u8>,   // distance code lengths
}
```

The optimal parser takes a `&dyn CostModel`. The first iteration
uses `ShannonCostModel`; subsequent iterations use
`HuffmanCostModel` built from the previous iteration's output.

### Termination

Stop when:
- Iteration count reaches 3, OR
- Output size change between iterations < 0.5%

Track the best output across all iterations (in case iteration N+1
overshoots and produces worse output).

## Acceptance criteria

- [ ] CSV 100KB Q5 improves by 1-2 percentage points.
- [ ] English text 500KB Q5 improves by 0.5-1 percentage point.
- [ ] No regression on binary data.
- [ ] All 86 brotli tests pass.
- [ ] Per-iteration runtime < 2x single-pass runtime.

## Why this matters

The current DP makes globally optimal decisions for an approximate
cost. Refining with actual Huffman costs lets it find cheaper
sequences that the approximation couldn't see. This is the
technique used by the C reference (Zopfli-style) and is responsible
for its remaining edge on text data.
