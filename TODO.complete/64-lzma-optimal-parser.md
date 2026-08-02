# 64 — LZMA optimal parser

## Gap

The current LZMA encoder uses a **lazy** (look-ahead-1) parser. This
finds decent matches but leaves 3-8% compression ratio on the table
versus an optimal parser, which uses dynamic programming to find the
global minimum-cost parse.

For LimniFS targets (LZMA L6 ≤ 16% on enwik8), the lazy parser is
estimated to produce ~17-18% ratio; the optimal parser is needed to
hit the ≤16% target.

## Algorithm (from xz-utils `lzma_encoder_optimum_fast.c`)

1. **Price table** — pre-compute the cost (bits) of encoding:
   - Each literal value (0-255) with/without match context.
   - Each (length, distance) pair, indexed by length slot and
     distance slot.
2. **DP table** — `opt[i]` = (cost, back-pointer) for the cheapest
   parse of input[0..i].
3. **Forward pass** — for each position `i`:
   - Find all matches at `i` using the match finder.
   - For each match (len, dist):
     - Update `opt[i + len]` if `opt[i].cost + price(len, dist)` is
       cheaper.
   - Also consider the literal option: `opt[i + 1] = opt[i].cost +
     price(literal)`.
4. **Backtrack** from `opt[input.len()]` to reconstruct the parse.

## Files

- `omnizip-lzma/src/encoder/optimal.rs` — the DP parser.
- `omnizip-lzma/src/encoder/prices.rs` — cost computation. Port
  directly from `~/src/external/xz-utils/src/liblzma/lzma/lzma_encoder_optimum_normal.c`.

## Complexity

- O(n · max_match_len) worst case, but typically O(n · 4) because
  most positions have few long matches.
- For a 1 MiB input: ~4M operations. Acceptable.

## Test strategy

- Highly repetitive input: optimal should match lazy (both find the
  maximal match).
- Slightly varying input (source code): optimal should beat lazy by
  3-8%.
- Round-trip via `Lzma1Decoder`.
- Determinism: same input → same output across runs.
