# 61 — FLAC FIXED subframe encoder

## Gap

VERBATIM gives functional FLAC output but no compression. FIXED
predictors (orders 0-4) encode each sample as a residual after
subtracting a fixed-coefficient prediction:

```
residual[i] = sample[i] - prediction[i]
prediction[i] = Σ coeff[k] * sample[i-1-k]   (k = 0..order-1)
```

Coefficients depend only on `order` (not on the data):

| order | coefficients |
|-------|-------------|
| 0     | (none) — residual = sample |
| 1     | [1] |
| 2     | [2, -1] |
| 3     | [3, -3, 1] |
| 4     | [4, -6, 4, -1] |

Order 0 is equivalent to VERBATIM but with Rice-coded residuals.

## Implementation

1. `omnizip-flac/src/encoder/fixed.rs` — choose best order (0..=4) by
   minimising sum of |residual|.
2. Encode warm-up samples (first `order` samples, stored raw).
3. Encode residuals via partitioned Rice coding (see task 62).

## Test strategy

- Sine wave input: FIXED order 1 should compress well (residuals are
  small deltas).
- DC signal: FIXED order 0 → residuals mostly 0, Rice code → 1 bit
  each.
- Compare output size against VERBATIM.
