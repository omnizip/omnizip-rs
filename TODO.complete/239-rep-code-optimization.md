# 239 — Repeat Offset (Rep Code) Optimization for Brotli

- **Priority:** P1 (significant ratio win on structured data)
- **Crate:** `omnizip-brotli`
- **Depends on:** none
- **Estimated effort:** 2 days

## Problem

The from_spec encoder tracks repeat offsets (rep codes) but only
checks `rep_offsets[0]` at each position. The C reference checks
ALL repeat offsets and uses the shortest code when a rep match
is found.

For CSV data with regular row spacing (~100 bytes), the distance
100 repeats for every row. Using rep code 0 (1-bit encoding) for
these repeated distances saves 8-12 bits per match compared to
explicit distance encoding.

## Current state

The parser checks rep0:
```rust
let rep0 = seq_store.rep_offsets[0];
if rep0 > 0 && ip > rep0 as usize {
    if src[ip..ip+4] == src[ip-rep0..ip-rep0+4] {
        // Found repcode match
    }
}
```

But it doesn't check rep1, rep2, or rep3. And it doesn't prefer
rep matches over regular matches when they're cheaper to encode.

## Design

1. Check all 3 repeat offsets at each position
2. When a rep match is found, prefer it over a regular match of
   the same length (rep codes are cheaper to encode)
3. Track the distance history more aggressively: after each match,
   update rep offsets even if the match wasn't a rep match
4. Use the Brotli distance code system: codes 0-15 are short/rep
   codes, requiring fewer bits than long-form distance codes

## Impact

For CSV with ~200K rows at distance ~100:
- Each row saves ~10 bits (rep code vs explicit distance)
- Total: 200K × 10 bits = 250KB savings
- On 20MB input: 250KB / 20MB = 1.25% ratio improvement

## Acceptance criteria

- [ ] All 3 repeat offsets checked at each position
- [ ] Rep matches preferred over regular matches of same length
- [ ] CSV ratio improvement >= 2%
- [ ] No speed regression (rep checks are O(1))
