# 68 — ZSTD length-limited Huffman (package-merge)

## Gap

The current Huffman encoder uses an ad-hoc redistribution heuristic
(`limit_lengths` in `huffman/encoder.rs`) to cap code lengths at
`HUF_TABLELOG_MAX = 11`. This is not optimal — it can produce codes
1-2 bits longer than the true minimum.

The C reference uses the **package-merge algorithm** (Larmore &
Hirschberg, 1990) which guarantees optimal length-limited codes in
O(n·L) time where n is the alphabet size and L is the length limit.

## Impact

For highly skewed distributions (e.g. 99% one symbol + 1% others),
the ad-hoc heuristic wastes ~0.1 bits/symbol. For typical
distributions the impact is <0.5%. Switching to package-merge would
tighten ratios by ~0.5-1%.

## Algorithm

```
package_merge(freqs, max_len):
  # coins[s] = list of (weight, boundary) for symbol s, sorted by weight
  coins = [(freqs[i], {i}) for i in 0..n]
  prev = coins
  for L in 1..max_len:
    merged = merge(coins, prev)   # merge + sort by weight
    prev = take(merged, 2n - 2)   # keep best 2n-2
  # Backtrack: each coin in prev corresponds to a symbol being at
  # level L. Increment its code length.
  lengths = [0] * n
  for L, coin in enumerate(reversed(prev)):
    for symbol in coin.boundary:
      lengths[symbol] += 1
  return lengths
```

## Files

- `omnizip-zstd/src/huffman/package_merge.rs` — new module.
- Replace `limit_lengths` call in `huffman/encoder.rs::build_weights`.

## Test strategy

- Compare output weight distribution against C reference for
  `enwik8` first 100K bytes.
- Verify Kraft inequality holds exactly (sum of 2^(w-1) = 2^L).
- Verify max code length ≤ 11.
