# 63 — FLAC partitioned Rice residual encoder

## Gap

FLAC encodes residuals (after FIXED or LPC prediction) using
**partitioned Rice coding**: the residual array is split into
partitions, each with its own Rice parameter `k`. This is needed by
both FIXED (task 61) and LPC (task 62) encoders.

## Algorithm

1. **Partition order**: 0..=15. `n_parts = 1 << order`. Higher orders
   allow adapting `k` to local statistics.
2. **For each partition**, find optimal `k`:
   - For each candidate k (0..=14):
     - Encode each residual as `(q, r)` where `q = |residual| >> k`
       and `r = |residual| & ((1<<k)-1)`.
     - Cost = unary(q) + binary(r, k).
   - Pick k minimising total cost.
3. **Escape code** (k = 15): store residuals raw (escape used when
   partition has very few residuals or very high entropy).

## Wire format

```
partition_order (4 bits, 0..=15)
for each partition:
  rice_parameter (4 bits, 0..=14, or 15 = escape)
  if escape:
    escape_bps (5 bits)
    raw residuals (escape_bps bits each)
  else:
    rice-coded residuals:
      for each residual:
        unary(q + sign) + binary(r, k)
```

## Files

- `omnizip-flac/src/encoder/rice.rs` — Rice encoder.
- Use `omnizip-flac/src/rice.rs` (decoder) as the reference for the
  bit layout.

## Test strategy

- All-zero residuals: k=0 → each residual is 1 bit.
- Sine wave residuals: k should adapt across partitions.
- Round-trip through existing `rice::decode_partitioned_rice`.
