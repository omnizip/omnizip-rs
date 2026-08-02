# 62 — FLAC LPC subframe encoder

## Gap

FIXED predictors use hardcoded coefficients. LPC (Linear Predictive
Coding) finds the optimal coefficients for the data via
autocorrelation, achieving 5-15% better compression than FIXED on
tonal audio (music, speech).

## Algorithm

1. **Window** the first 4-32K samples (use order up to 32).
2. **Compute autocorrelation** at lags 0..order.
3. **Levinson-Durbin recursion** — solve for LPC coefficients.
4. **Quantise** coefficients to `qlp_coeff_precision` bits (typically
   5-12). Compute shift needed for exact reconstruction.
5. **Compute residuals** using quantised coefficients.
6. **Choose order** by trying orders 1..32, picking the one that
   minimises Rice-coded residual size.

## Wire format

```
LPC subframe:
  order (5 bits, encoded in subframe type)
  qlp_coeff_precision (5 bits, 0 = 4-bit escape)
  qlp_shift (5 bits, signed)
  qlp_coeff[order] (qlp_coeff_precision bits each, signed)
  warm_up[order] (bps bits each)
  partitioned Rice residual (see task 63)
```

## Files

- `omnizip-flac/src/encoder/lpc.rs` — autocorrelation + Levinson-Durbin.
- `omnizip-flac/src/encoder/quantise.rs` — coefficient quantisation.

## Complexity

~500-800 LOC. Autocorrelation is O(n·order), Levinson-Durbin is
O(order²). For order=32 and 4K blocks, total is ~128K ops/block.

## Test strategy

- Synthetic sine wave → LPC order 2 should give near-zero residuals.
- Real audio fixture (e.g. `eagle.flac` 44.1 kHz stereo) → verify
  ratio is within 5% of libFLAC.
