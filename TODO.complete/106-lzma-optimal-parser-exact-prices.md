# 106 — LZMA optimal parser: full price integration

**Priority:** Medium — feature gap
**Source:** LimniFS proposal `omnizip-proposals/lzma-optimal-parser.md`
**Status:** ⏳ Pending — TODO 64 partial; this completes it.

## Problem

TODO 64 marked COMPLETED: an optimal parser exists
(`encoder/optimal.rs`) and produces 3–8% better ratio than the lazy
parser. **However**, the price estimates are simplified:

> The price estimates are simplified (no full probability-model
> integration). The C reference's
> `lzma_encoder_optimum_normal.c` has more accurate prices that
> account for state transitions, match-byte context, and
> length/distance slot probabilities. Upgrading to exact prices
> would give an additional 1–2% ratio improvement.

`omnizip-lzma 0.13.1` produces output 5–10% larger than the `xz`
CLI on text fixtures. The remaining gap is mostly the simplified
price model.

## Current state

`encoder/optimal.rs` (~250 LOC):

- `optimal_parse_actions`: forward DP + backtracking.
- Prices in 1/8-bit units matching C reference convention.
- `ParseAction` enum: `Literal(u8)`, `Match { distance, length }`,
  `Rep0Match { length }`.

Prices are constants:
- Literal: ~64 units
- Match: ~96-200 units
- Rep0 match: ~72 units

The C reference uses **state-conditioned** prices:

- Literal price depends on `(prev_state, prev_byte, current_byte)`.
- Match price depends on `(state, length_slot, distance_slot)`.
- Rep0 price depends on `(state, length_slot)`.

## Phased delivery

### Phase 1 — Extract LzmaProbState (2 days)

- Move the probability state machine out of the encoder into its
  own module.
- `literal_cost(state, prev_byte, byte) -> u32`
- `match_cost(state, length, distance) -> u32`
- `rep0_cost(state, length) -> u32`

### Phase 2 — Optimal parse with exact prices (3 days)

- Replace the constant prices in `optimal.rs` with the new functions.
- Validate ratio improvement on Calgary `book1` (768 KB).
- Acceptance: ≥ 1% improvement over current optimal parser.

### Phase 3 — Wire level → parser (1 day)

```rust
pub fn lzma_compress_with_parser(
    plaintext: &[u8],
    level: LzmaLevel,
    parser: LzmaParser,  // Lazy vs Optimal
) -> Result<Vec<u8>, LzmaError>;
```

Map `level ≥ 6` to optimal parser; `level < 6` to lazy (faster).

## Acceptance criteria

- [ ] Calgary `book1` (768 KB): optimal-with-exact-prices is
      ≥ 5% smaller than current lazy parser.
- [ ] Calgary `paper1` (53 KB): ≥ 3% smaller.
- [ ] Encode time within 3× of lazy (optimal parsing is slow).
- [ ] Decoder byte-identical (no wire-format change).

## Why LimniFS cares

LimniFS's `max-ratio` profile routes text through a tournament that
includes PPMd7, Brotli q11, and LZMA. LZMA losing 5–10% to `xz` CLI
means Brotli or PPMd wins on text — but PPMd is slow to decode, and
Brotli q11 is slow to encode. Optimal-parser LZMA gives the best text
ratio with reasonable encode/decode speed.

## Effort estimate

6 days (per phased delivery above).

## Related

- omnizip-rs TODO 64 (optimal parser foundation — landed).
- Igor Pavlov LZMA SDK: https://www.7-zip.org/sdk.html
- LZMA spec: `LZMA spec.txt` in the 7-Zip source distribution.
- LimniFS proposal `omnizip-proposals/lzma-optimal-parser.md`.
