# 201 — Brotli Optimal Parsing (Dynamic Programming)

- **Priority:** P2 (5% ratio win, significant complexity)
- **Crate:** `omnizip-brotli`
- **Depends on:** none
- **Estimated effort:** 1 week

## Goal

Replace the current greedy/lazy match parser with an **optimal
parser** that uses dynamic programming to find the command sequence
with minimum total bit cost.

## Background

The current parser at quality ≥ 4 uses **lazy matching**: when a match
is found, peek one position ahead and defer if the next match is
longer. This is O(n) but suboptimal.

An optimal parser (RFC 7932 doesn't mandate one, but the reference
encoder uses it at quality ≥ 10) computes, for each position, the
minimum-cost path from that position to the end. The cost model
accounts for:

- Literal cost: Huffman code length for the byte in context
- Match cost: command symbol cost + distance cost + extra bits
- Insert/copy overhead: the command symbol itself costs bits

## Scope

1. **Cost model** (2 days): estimate the bit cost of each possible
   command at each position. Requires provisional Huffman tables
   (build once, use for cost estimation).

2. **DP backward pass** (3 days): for each position from end to start,
   compute the minimum-cost path. State: current position. Transitions:
   - Insert 1 literal (cost = literal Huffman cost)
   - Copy match of length L at distance D (cost = command + distance)

3. **Path reconstruction** (1 day): forward pass to reconstruct the
   optimal command sequence from the DP table.

4. **Quality gating** (1 day): use optimal parsing only at q ≥ 10.
   Lower levels keep lazy/greedy for speed.

## Acceptance criteria

- [ ] Optimal parser produces ≤ lazy parser output size on all inputs
- [ ] Ratio improvement ≥ 3% on diverse text at q ≥ 10
- [ ] Encode speed ≥ 5 MB/s at q=10 (optimal is inherently slower)
- [ ] Round-trip correctness preserved
- [ ] Deterministic output

## Implementation plan

### New module: `omnizip-brotli/src/encoder/optimal_parser.rs`

```rust
pub struct OptimalParser<'a> {
    input: &'a [u8],
    mf: &'a mut HashChainMatchFinder<'a>,
    cost_model: CostModel,
}

struct CostModel {
    lit_lengths: Vec<u8>,    // per-byte Huffman lengths (provisional)
    cmd_costs: Vec<u32>,     // per-command-symbol cost
    dist_costs: Vec<u32>,    // per-distance-symbol cost
}

impl OptimalParser<'_> {
    /// Backward DP: compute min_cost[i] = minimum bits from i to end.
    fn backward_pass(&mut self) -> Vec<f32> { ... }

    /// Forward reconstruction: walk the DP table to emit commands.
    fn forward_pass(&self, costs: &[f32]) -> Vec<Command> { ... }
}
```

### Integration

Add a `ParsingStrategy` enum:
```rust
pub enum ParsingStrategy {
    Greedy,
    Lazy,
    Optimal,
}
```

Quality → strategy mapping:
- q 0–3: Greedy
- q 4–9: Lazy
- q 10–11: Optimal

## Test plan

- Unit test: optimal parser produces ≤ lazy output on text/binary
- Unit test: cost model estimates are within 10% of actual sizes
- Integration: full round-trip at q=11
- Benchmark: encode speed at q=10 vs q=9 (should be slower but better ratio)

## References

- RFC 7932 §5 (insert-and-copy commands)
- Upstream: `brotli/c/enc/backward_references.c:ZopfliCostModel`
- zlib's `deflate_medium` / `deflate_slow` for comparison
