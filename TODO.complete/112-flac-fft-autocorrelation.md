# TODO 112: FLAC FFT-based autocorrelation

## Problem

`omnizip-flac/src/encoder/lpc.rs::autocorrelate` is
`O(max_order × N)`:

```rust
for lag in 0..=max_order {
    for i in lag..n {
        sum += samples[i] as f64 * samples[i - lag] as f64;
    }
}
```

For a 16-order LPC analysis on a 4 KiB block: 16 × 4096 = 65 K mults.
That's small in absolute terms, but the cost grows linearly with both
`max_order` and `N`. For the proposed 32-order extension (TODO 105)
on 64 KiB blocks: 32 × 65 K = 2 M mults per subframe.

## Proposed fix

Autocorrelation via FFT:

```text
ACF = IFFT(|FFT(signal)|^2)
```

For a real signal of length N:
1. Zero-pad to next power of two ≥ `2N - 1` (avoid wrap-around).
2. Compute real FFT (rFFT).
3. Square magnitudes element-wise.
4. Inverse rFFT.
5. Take first `max_order + 1` outputs.

Total cost: `O(N log N)` for any `max_order`.

## Implementation plan

1. Add a pure-Rust iterative Cooley-Tukey radix-2 rFFT to
   `omnizip-flac/src/encoder/fft.rs`. Twiddle factors pre-computed.
2. Gate behind a `fft-acf` cargo feature so the scalar path remains
   the default for determinism-sensitive callers.
3. Verify bit-exact (within `f64::EPSILON`) ACF output vs scalar path.
4. Levinson-Durbin recursion is already `O(max_order²)`, so once ACF
   is FFT-driven the recursion is the dominant cost — fine for
   `max_order ≤ 32`.

## Acceptance criteria

- [ ] `fft-acf` feature compiles, ACF matches scalar within 1e-9.
- [ ] Same LPC coefficients (after quantisation) on 1 KiB+ blocks.
- [ ] Round-trip tests pass.
- [ ] Bench: 10× or more speed-up on ACF for 64 KiB blocks.

## Priority

P1 — second half of the FLAC 10× gap. Do TODO 111 first (bigger win).
