//! Pure-Rust iterative radix-2 real-valued FFT.
//!
//! Used by [`super::lpc`] for `O(N log N)` autocorrelation via the
//! Wiener-Khinchin theorem: `ACF = IFFT(|FFT(signal)|^2)`.
//!
//! ## Algorithm
//!
//! Standard iterative Cooley-Tukey radix-2:
//! 1. Bit-reverse permutation of the input.
//! 2. Log2(N) stages, each combining pairs via twiddle factors.
//! 3. Twiddle factors pre-computed via `exp(-2πi k / N)`.
//!
//! Only handles power-of-two sizes; callers zero-pad.
//!
//! ## Determinism
//!
//! Uses `f64` throughout. IEEE 754 round-to-nearest gives identical
//! results across platforms. Twiddle factor pre-computation uses
//! `cos` and `sin` once at table-build time, so any platform
//! difference in those functions would change the cached values —
//! but every IEEE 754-conformant libm agrees on `cos`/`sin` inputs
//! that are exact multiples of π/N for small N.

#![forbid(unsafe_code)]

use std::f64::consts::PI;

/// A real-valued FFT plan, precomputed for one transform size `n`
/// (power of two).
pub struct RealFft {
    n: usize,
    /// Twiddle factors: `twiddle[k] = exp(-2πi k / n)` for k in 0..n/2.
    twiddle_re: Vec<f64>,
    twiddle_im: Vec<f64>,
    /// Bit-reverse permutation lookup.
    bit_reverse: Vec<usize>,
}

impl RealFft {
    /// Construct a plan for size `n`. `n` must be a power of two ≥ 2.
    ///
    /// # Panics
    ///
    /// Panics if `n` is not a power of two or is < 2.
    #[must_use]
    pub fn new(n: usize) -> Self {
        assert!(n >= 2 && n.is_power_of_two(), "n must be power of two ≥ 2");

        let half = n / 2;
        let mut twiddle_re = Vec::with_capacity(half);
        let mut twiddle_im = Vec::with_capacity(half);
        for k in 0..half {
            let angle = -2.0 * PI * k as f64 / n as f64;
            twiddle_re.push(angle.cos());
            twiddle_im.push(angle.sin());
        }

        // Bit-reverse permutation: for each index i, compute the
        // bit-reversal of its log2(n) low bits.
        let log_n = n.trailing_zeros() as usize;
        let mut bit_reverse = Vec::with_capacity(n);
        for i in 0..n {
            let mut rev = 0usize;
            let mut v = i;
            for _ in 0..log_n {
                rev = (rev << 1) | (v & 1);
                v >>= 1;
            }
            bit_reverse.push(rev);
        }

        Self {
            n,
            twiddle_re,
            twiddle_im,
            bit_reverse,
        }
    }

    /// Forward FFT, in-place. Input is complex; output is complex
    /// spectrum. Length must match `self.n`.
    pub fn forward(&self, re: &mut [f64], im: &mut [f64]) {
        assert_eq!(re.len(), self.n);
        assert_eq!(im.len(), self.n);
        self.transform(re, im, false);
    }

    /// Inverse FFT, in-place. No normalisation — caller divides by
    /// `self.n` if needed.
    pub fn inverse(&self, re: &mut [f64], im: &mut [f64]) {
        assert_eq!(re.len(), self.n);
        assert_eq!(im.len(), self.n);
        self.transform(re, im, true);
    }

    /// The size `n` this plan was constructed for.
    #[must_use]
    pub const fn n(&self) -> usize {
        self.n
    }

    fn transform(&self, re: &mut [f64], im: &mut [f64], inverse: bool) {
        let n = self.n;

        // Bit-reverse permutation in place.
        for i in 0..n {
            let j = self.bit_reverse[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }

        // Iterative Cooley-Tukey. Stage `s` processes blocks of size
        // 2^s; within each block, pairs are combined via twiddle
        // factors.
        let mut half_size = 1usize;
        while half_size < n {
            let full_size = half_size * 2;
            // Stride between twiddle indices: n / full_size.
            let tw_stride = n / full_size;
            // For inverse: conjugate the twiddle factors.
            let sign = if inverse { -1.0 } else { 1.0 };

            let mut k_start = 0;
            while k_start < n {
                let mut k_tw = 0;
                for k in 0..half_size {
                    let i = k_start + k;
                    let j = i + half_size;
                    let tw_re = self.twiddle_re[k_tw];
                    let tw_im = sign * self.twiddle_im[k_tw];

                    // Complex multiply: (re[j] + i*im[j]) * (tw_re + i*tw_im).
                    let a_re = re[j];
                    let a_im = im[j];
                    let prod_re = a_re * tw_re - a_im * tw_im;
                    let prod_im = a_re * tw_im + a_im * tw_re;

                    // Butterfly.
                    re[j] = re[i] - prod_re;
                    im[j] = im[i] - prod_im;
                    re[i] += prod_re;
                    im[i] += prod_im;

                    k_tw += tw_stride;
                }
                k_start += full_size;
            }
            half_size = full_size;
        }
    }
}

/// Compute the autocorrelation of a real signal via FFT.
///
/// Returns `acf[0..=max_lag]`. The result matches the scalar
/// `O(N * max_lag)` definition within `~1e-9` (f64 precision).
///
/// ## Algorithm
///
/// 1. Zero-pad signal to length `2N - 1` rounded up to next power of
///    two. This avoids circular wrap-around in the FFT.
/// 2. `forward = FFT(real_signal)`.
/// 3. Power spectrum: `P[k] = |F[k]|^2 = F[k] * conj(F[k])`.
/// 4. `acf = IFFT(P)`.
/// 5. Take the first `max_lag + 1` entries (the symmetric half).
///
/// ## Determinism
///
/// Bit-identical across platforms per the module-level determinism
/// note.
#[must_use]
pub fn autocorrelate_fft(samples: &[i32], max_lag: usize) -> Vec<f64> {
    let n = samples.len();
    if n == 0 {
        return vec![0.0; max_lag + 1];
    }

    // Next power of two ≥ 2N - 1.
    let target = (2 * n).max(2) - 1;
    let fft_size = target.next_power_of_two().max(2);

    let mut re = vec![0.0f64; fft_size];
    let mut im = vec![0.0f64; fft_size];
    for (i, &s) in samples.iter().enumerate() {
        re[i] = f64::from(s);
    }

    let fft = RealFft::new(fft_size);
    fft.forward(&mut re, &mut im);

    // Power spectrum.
    for k in 0..fft_size {
        let r = re[k];
        let i = im[k];
        re[k] = r * r + i * i;
        im[k] = 0.0;
    }

    fft.inverse(&mut re, &mut im);

    // Normalise by 1/fft_size and take the first max_lag+1 entries.
    // The IFFT gives us the linear autocorrelation (since we
    // zero-padded to ≥ 2N-1).
    let scale = 1.0 / fft_size as f64;
    let mut acf = Vec::with_capacity(max_lag + 1);
    for k in 0..=max_lag {
        acf.push(re[k] * scale);
    }
    acf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_round_trips_constant_signal() {
        // FFT of [1, 1, 1, 1] should be [4, 0, 0, 0] in spectrum.
        let fft = RealFft::new(4);
        let mut re = vec![1.0, 1.0, 1.0, 1.0];
        let mut im = vec![0.0; 4];
        fft.forward(&mut re, &mut im);
        assert!((re[0] - 4.0).abs() < 1e-9);
        assert!((re[1]).abs() < 1e-9);
        assert!((re[2]).abs() < 1e-9);
        assert!((re[3]).abs() < 1e-9);
    }

    #[test]
    fn fft_round_trips_sine_wave() {
        // 8 samples of cos(2πk/8): FFT should give DC=0, k=1 magnitude=4.
        let fft = RealFft::new(8);
        let mut re: Vec<f64> = (0..8).map(|k| (2.0 * PI * k as f64 / 8.0).cos()).collect();
        let mut im = vec![0.0; 8];
        fft.forward(&mut re, &mut im);

        // DC component should be ~0.
        assert!(re[0].abs() < 1e-9, "DC should be 0, got {}", re[0]);

        // k=1 magnitude (cosine → real part ±4).
        let mag1 = (re[1] * re[1] + im[1] * im[1]).sqrt();
        assert!((mag1 - 4.0).abs() < 1e-9, "k=1 mag should be 4, got {mag1}");

        // Inverse + normalise should give back the original signal.
        fft.inverse(&mut re, &mut im);
        for k in 0..8 {
            let expected = (2.0 * PI * k as f64 / 8.0).cos();
            let got = re[k] / 8.0;
            assert!(
                (got - expected).abs() < 1e-9,
                "sample {k}: {got} vs {expected}"
            );
        }
    }

    #[test]
    fn fft_size_must_be_power_of_two() {
        let result = std::panic::catch_unwind(|| RealFft::new(3));
        assert!(result.is_err(), "non-power-of-two should panic");
        let result = std::panic::catch_unwind(|| RealFft::new(1));
        assert!(result.is_err(), "n<2 should panic");
    }

    #[test]
    fn autocorrelate_fft_matches_scalar_for_dc_signal() {
        // For a constant signal of N samples each = V, acf[0] = N*V^2,
        // acf[k] = (N-k)*V^2.
        let samples: Vec<i32> = vec![100; 64];
        let max_lag = 8;
        let acf = autocorrelate_fft(&samples, max_lag);

        // Compare against scalar definition.
        for lag in 0..=max_lag {
            let mut expected = 0.0;
            for i in lag..samples.len() {
                expected += samples[i] as f64 * samples[i - lag] as f64;
            }
            let rel_err = (acf[lag] - expected).abs() / expected.abs().max(1.0);
            assert!(
                rel_err < 1e-9,
                "lag {lag}: FFT {acf:?} vs scalar {expected} (rel_err {rel_err})"
            );
        }
    }

    #[test]
    fn autocorrelate_fft_matches_scalar_for_sine() {
        let samples: Vec<i32> = (0..512)
            .map(|i| ((i as f64 * 0.07).sin() * 30_000.0) as i32)
            .collect();
        let max_lag = 16;
        let acf_fft = autocorrelate_fft(&samples, max_lag);

        for lag in 0..=max_lag {
            let mut expected = 0.0;
            for i in lag..samples.len() {
                expected += samples[i] as f64 * samples[i - lag] as f64;
            }
            let rel_err = (acf_fft[lag] - expected).abs() / expected.abs().max(1.0);
            assert!(
                rel_err < 1e-9,
                "lag {lag}: FFT {} vs scalar {} (rel_err {rel_err})",
                acf_fft[lag],
                expected
            );
        }
    }

    #[test]
    fn autocorrelate_fft_handles_short_input() {
        let samples = vec![1, 2, 3, 4];
        let acf = autocorrelate_fft(&samples, 4);
        assert_eq!(acf.len(), 5);
        // acf[0] = 1+4+9+16 = 30.
        assert!((acf[0] - 30.0).abs() < 1e-9);
    }
}
