//! SIMD-accelerated inner loops for the LPC encoder.
//!
//! Behind the `simd-lpc` cargo feature (default off). Uses [`wide`]
//! for portable 256-bit SIMD on stable Rust.
//!
//! ## What's vectorised
//!
//! - [`residuals_i32x8`]: the per-sample prediction loop
//!   `sum += qlpc[j] * samples[i-1-j]` is the hottest single loop
//!   in LPC encoding. We unroll it 8 samples at a time using
//!   `i32x8` multiply-add.
//!
//! - [`autocorrelation_lag`]: the lag-`k` inner product
//!   `sum += samples[i] * samples[i-k]` is the second-hottest.
//!   We vectorise the inner-product step (not the outer lag loop,
//!   since lags are sequential by data dependency).
//!
//! Both fall back to the scalar paths when the feature is disabled —
//! the API is identical so callers don't need to branch.

#![forbid(unsafe_code)]

#[cfg(feature = "simd-lpc")]
use wide::i32x8;

/// Compute `samples[i] - predict(qlpc, samples, i, shift)` for every
/// `i` in `order..samples.len()`. Result has length `samples.len() - order`.
///
/// `qlpc` has length `order`; `qlpc[j]` multiplies `samples[i-1-j]`.
/// Wrapping arithmetic — mirrors libFLAC's decoder exactly.
#[cfg(feature = "simd-lpc")]
pub(crate) fn residuals_i32x8(qlpc: &[i32], shift: i8, samples: &[i32], order: usize) -> Vec<i32> {
    let n_out = samples.len() - order;
    let mut out = vec![0i32; n_out];
    let lanes = 8usize;

    let mut i = 0usize;
    while i + lanes <= n_out {
        let pos = i + order;
        // Load 8 consecutive samples using `new([T; N])`.
        let cur = i32x8::new([
            samples[pos],
            samples[pos + 1],
            samples[pos + 2],
            samples[pos + 3],
            samples[pos + 4],
            samples[pos + 5],
            samples[pos + 6],
            samples[pos + 7],
        ]);

        let mut predicted = i32x8::splat(0);
        for j in 0..order {
            let hist_start = pos - 1 - j;
            let hist = i32x8::new([
                samples[hist_start],
                samples[hist_start + 1],
                samples[hist_start + 2],
                samples[hist_start + 3],
                samples[hist_start + 4],
                samples[hist_start + 5],
                samples[hist_start + 6],
                samples[hist_start + 7],
            ]);
            let coeff = i32x8::splat(qlpc[j]);
            predicted += coeff * hist;
        }

        let predicted_shifted = if shift >= 0 {
            predicted >> i32x8::splat(i32::from(shift))
        } else {
            predicted << i32x8::splat(i32::from(-shift))
        };
        let residual = cur - predicted_shifted;
        let res_arr = residual.to_array();
        out[i..i + lanes].copy_from_slice(&res_arr);
        i += lanes;
    }

    // Tail: scalar.
    while i < n_out {
        let pos = i + order;
        let mut predicted: i32 = 0;
        for j in 0..order {
            predicted = predicted.wrapping_add(qlpc[j].wrapping_mul(samples[pos - 1 - j]));
        }
        let predicted_shifted = if shift >= 0 {
            predicted >> shift
        } else {
            predicted << (-shift)
        };
        out[i] = samples[pos].wrapping_sub(predicted_shifted);
        i += 1;
    }

    out
}

/// Scalar fallback used when `simd-lpc` is disabled. Identical
/// arithmetic semantics to the SIMD path so callers don't need to
/// branch on the feature.
#[cfg(not(feature = "simd-lpc"))]
pub(crate) fn residuals_i32x8(qlpc: &[i32], shift: i8, samples: &[i32], order: usize) -> Vec<i32> {
    let n_out = samples.len() - order;
    let mut out = Vec::with_capacity(n_out);
    for k in 0..n_out {
        let pos = k + order;
        let mut predicted: i32 = 0;
        for j in 0..order {
            predicted = predicted.wrapping_add(qlpc[j].wrapping_mul(samples[pos - 1 - j]));
        }
        let predicted_shifted = if shift >= 0 {
            predicted >> shift
        } else {
            predicted << (-shift)
        };
        out.push(samples[pos].wrapping_sub(predicted_shifted));
    }
    out
}

/// Compute `sum(samples[i] * samples[i-lag]) for i in lag..n`.
///
/// The outer lag loop is sequential (each lag's result is needed for
/// Levinson-Durbin), but the inner product is vectorisable.
#[cfg(feature = "simd-lpc")]
pub(crate) fn autocorrelation_lag(samples: &[i32], lag: usize) -> f64 {
    let n = samples.len();
    if lag >= n {
        return 0.0;
    }
    let count = n - lag;
    let lanes = 8usize;

    // Vectorised inner product: 8 i32s per iteration, accumulate into
    // i64x8 to avoid overflow.
    let mut acc = [0i64; 8];
    let mut i = 0usize;
    while i + lanes <= count {
        let a = [
            i64::from(samples[i + lag]),
            i64::from(samples[i + lag + 1]),
            i64::from(samples[i + lag + 2]),
            i64::from(samples[i + lag + 3]),
            i64::from(samples[i + lag + 4]),
            i64::from(samples[i + lag + 5]),
            i64::from(samples[i + lag + 6]),
            i64::from(samples[i + lag + 7]),
        ];
        let b = [
            i64::from(samples[i]),
            i64::from(samples[i + 1]),
            i64::from(samples[i + 2]),
            i64::from(samples[i + 3]),
            i64::from(samples[i + 4]),
            i64::from(samples[i + 5]),
            i64::from(samples[i + 6]),
            i64::from(samples[i + 7]),
        ];
        for k in 0..8 {
            acc[k] += a[k] * b[k];
        }
        i += lanes;
    }

    // Tail: scalar.
    let mut sum: i64 = acc.iter().sum();
    while i < count {
        sum += i64::from(samples[i + lag]) * i64::from(samples[i]);
        i += 1;
    }

    sum as f64
}

/// Scalar fallback for [`autocorrelation_lag`].
#[cfg(not(feature = "simd-lpc"))]
pub(crate) fn autocorrelation_lag(samples: &[i32], lag: usize) -> f64 {
    let n = samples.len();
    if lag >= n {
        return 0.0;
    }
    let mut sum: i64 = 0;
    for i in lag..n {
        sum += i64::from(samples[i]) * i64::from(samples[i - lag]);
    }
    sum as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn residuals_simd_matches_scalar() {
        // Same input, both paths — verify identical output.
        let samples: Vec<i32> = (0..200)
            .map(|i| ((i as f64 * 0.1).sin() * 1000.0) as i32)
            .collect();
        let qlpc: Vec<i32> = vec![100, -200, 150, -50, 75, -25, 10, -5];
        let order = qlpc.len();
        let shift: i8 = 3;

        let got = residuals_i32x8(&qlpc, shift, &samples, order);

        // Reference scalar computation.
        let mut want = Vec::with_capacity(samples.len() - order);
        for k in 0..samples.len() - order {
            let pos = k + order;
            let mut predicted: i32 = 0;
            for j in 0..order {
                predicted = predicted.wrapping_add(qlpc[j].wrapping_mul(samples[pos - 1 - j]));
            }
            let predicted_shifted = predicted >> shift;
            want.push(samples[pos].wrapping_sub(predicted_shifted));
        }

        assert_eq!(got, want, "SIMD residual != scalar residual");
    }

    #[test]
    fn autocorrelation_simd_matches_scalar() {
        let samples: Vec<i32> = (0..300)
            .map(|i| ((i as f64 * 0.07).sin() * 5000.0) as i32)
            .collect();
        for lag in 0..16 {
            let got = autocorrelation_lag(&samples, lag);
            let mut want = 0.0;
            for i in lag..samples.len() {
                want += samples[i] as f64 * samples[i - lag] as f64;
            }
            let rel_err = (got - want).abs() / want.abs().max(1.0);
            assert!(
                rel_err < 1e-9,
                "lag {lag}: SIMD {got} != scalar {want} (rel_err {rel_err})"
            );
        }
    }

    #[test]
    fn residuals_handles_short_input() {
        // Input shorter than 8 samples — exercises the tail path.
        let samples = vec![100, 200, 300, 400, 500];
        let qlpc = vec![10, -20];
        let order = qlpc.len();
        let got = residuals_i32x8(&qlpc, 0, &samples, order);
        assert_eq!(got.len(), samples.len() - order);
    }
}
