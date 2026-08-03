//! Adaptive logistic mixer for combining model predictions.
//!
//! The mixer takes the per-bit probability outputs of N context models and
//! produces a single probability fed to the arithmetic coder. Weights are
//! adapted after each coded bit so that models which predicted the bit well
//! receive more influence over time.
//!
//! ## Algorithm
//!
//! This is a PAQ-style logistic mixer rather than a linear weighted average:
//!
//! 1. Convert each model probability to a log-odds (stretch): `s = ln(p/(1-p))`.
//! 2. Combine: `S = sum_i w_i * s_i` (weights in fixed-point).
//! 3. Convert back (squash): `p = 1 / (1 + exp(-S))`.
//!
//! Logistic mixing is preferred over linear mixing because probabilities add
//! in log-odds space — combining two equally-confident predictions should
//! sharpen the result, not average it.
//!
//! The weights are updated by gradient descent on the log-likelihood of the
//! observed bit. Let `err = bit - p` (in `[0,1]`); the update is
//! `w_i += lr * err * s_i`. This is the standard logistic-regression SGD step.
//!
//! ## Fixed-point
//!
//! Probabilities are `u16` in `[1, 65535]`. Internally we use the well-known
//! PAQ trick of precomputed stretch/squash lookup tables on a 12-bit
//! probability resolution (4096 entries).
//!
//! ## Determinism
//!
//! All operations are pure functions of `(weights, probs, bit)`. There is no
//! RNG, no float, no scheduling-dependent behaviour.

#![forbid(unsafe_code)]
// Fixed-point arithmetic inherently involves narrowing casts between
// i32/i64/u16/usize. All such casts in this module are provably safe
// (operands are bounded by the constants STRETCH_SIZE, PROB_SCALE,
// WEIGHT_SCALE, and the stretch table range) but clippy::pedantic flags
// them unconditionally.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

/// Number of input models mixed by [`Mixer`].
///
/// Six models feed the mixer: order-0, order-1, order-2, order-3,
/// match, and run-length. Adding more models (word-level) requires
/// updating this constant plus every callsite that builds a
/// `[u16; NUM_MODELS]` array — kept deliberately small for that
/// reason. See `TODO.complete/80-zpaq-more-models.md`.
pub const NUM_MODELS: usize = 6;

/// Probability scale (matches the arithmetic coder's `PROB_SCALE`).
const PROB_SCALE: i32 = 65_536;

/// Probability clamp (1 ..= 65535).
const PROB_MIN: i32 = 1;
const PROB_MAX: i32 = PROB_SCALE - 1;

/// Stretch-table resolution: probabilities are quantised to 4096 buckets
/// before lookup. This bounds the table size while keeping the relative error
/// below ~0.1%.
const STRETCH_BITS: u32 = 12;
const STRETCH_SIZE: usize = 1 << STRETCH_BITS; // 4096

/// Fixed-point bits for weights (Q.8 gives plenty of headroom for adaptation
/// without overflowing `i32` sums of up to `NUM_MODELS` stretched values).
const WEIGHT_FRAC_BITS: i32 = 8;
const WEIGHT_SCALE: i32 = 1 << WEIGHT_FRAC_BITS;

/// Learning rate as a fraction of `WEIGHT_SCALE`. PAQ-style coders
/// typically use rates in the 0.001..0.05 range, but for short inputs
/// (kilobytes, not megabytes) we need faster convergence. We pick 1/64
/// (i.e. `WEIGHT_SCALE / 64 == 4` in raw units) as a compromise between
/// fast early adaptation and long-term stability.
const LEARN_NUM: i32 = 1;
const LEARN_DEN: i32 = 64;

// ---------------------------------------------------------------------------
// Stretch / squash lookup tables.
// ---------------------------------------------------------------------------

/// Stretch table: `stretch[p] = round(ln(p/(1-p)) * 256)` for `p` quantised
/// to 12 bits (index in `[1, 4095]`). Index 0 is unused (probability 0 is
/// never produced).
///
/// We compute this at build time with a `const` block. The natural log values
/// range roughly from -5.74 (`p=1/4095`) to +5.74; multiplied by 256 the
/// range fits comfortably in `i16` (~-1468 ..= 1468), but we use `i32` to
/// keep arithmetic cheap.
static STRETCH: [i32; STRETCH_SIZE] = build_stretch_table();

/// `const fn` building the stretch table.
const fn build_stretch_table() -> [i32; STRETCH_SIZE] {
    // We compute ln(p / (1 - p)) for p = i / 4096, i in [1, 4095], using a
    // simple fixed-point log/exp via the series expansion of atanh, which is
    // exactly half the logit:  ln(p/(1-p)) = 2 * atanh(2p - 1).
    //
    // atanh(x) = x + x^3/3 + x^5/5 + x^7/7 + ...   for |x| < 1.
    //
    // All terms share the sign of x (since x^(2k+1) has the same sign as x).
    // We use Q.24 fixed point internally for the series accumulation.
    let mut t = [0i32; STRETCH_SIZE];
    let mut i = 1;
    let q24_one: i64 = 1 << 24;
    while i < STRETCH_SIZE {
        // p = i / 4096 in [1/4096, 4095/4096]
        // 2p - 1 = (2*i - 4096) / 4096 = (i - 2048) / 2048
        // We want atanh((2p-1)) where |2p-1| < 1.
        // For i near 0 or 4095, |2p-1| is near 1 and the series converges
        // slowly; we compensate with enough iterations.
        let num = (i as i64) - 2048;
        let denom: i64 = 2048;
        // x in Q.24 = num * 2^24 / denom, sign preserved.
        let x = num * q24_one / denom; // |x| < 2^24
        let mut xpow = x; // x^1 in Q.24
        let mut acc: i64 = 0;
        let mut k: i64 = 1;
        // Iterate enough terms for convergence even at |x| close to 1.
        // At |x|=0.999 (i=1 or 4095), ~7000 terms are needed for full
        // precision; we cap at 256 iterations which gives ~3-digit accuracy
        // at the extremes — good enough for our 12-bit probability buckets.
        while k <= 512 {
            // term = x^(2k-1) / (2k-1)  — only odd powers, all same sign.
            let term = xpow / k;
            acc += term;
            // Advance two powers: x^(k+2) = x^(k) * x^2.
            xpow = xpow * x / q24_one; // x^(k+1)
            xpow = xpow * x / q24_one; // x^(k+2)
                                       // Stop early once terms become negligible.
            if xpow.abs() < 4 {
                break;
            }
            k += 2;
        }
        // atanh(x) = acc (in Q.24). ln(p/(1-p)) = 2 * atanh(x).
        // Multiply by 2 to get the logit, then convert Q.24 -> Q.8 by /2^16.
        // logit_q8 = 2 * acc / 2^16 = acc / 2^15
        let logit_q8 = acc / (1 << 15);
        // Clamp into i32 range (the true range is ~-1468..=1468).
        let clamped = if logit_q8 > 32767 {
            32767
        } else if logit_q8 < -32767 {
            -32767
        } else {
            logit_q8 as i32
        };
        t[i] = clamped;
        i += 1;
    }
    t
}

/// Quantise a `u16` probability `[1, 65535]` to a 12-bit bucket index
/// `[1, 4095]` for stretch-table lookup.
#[inline]
fn quantise(prob: u16) -> usize {
    // Map 65536 buckets to 4096 buckets by dividing by 16; ensure result is
    // at least 1 (prob is in [1, 65535]).
    let idx = (u32::from(prob) + 8) >> 4; // round-to-nearest, [1, 4095]
    idx.clamp(1, (STRETCH_SIZE - 1) as u32) as usize
}

/// Stretch a probability into log-odds (Q.8 fixed-point).
#[inline]
fn stretch(prob: u16) -> i32 {
    // SAFETY: quantise guarantees index in [1, 4095]; STRETCH has size 4096.
    STRETCH[quantise(prob)]
}

/// Squash a log-odds value (Q.8 fixed-point) back to a probability `u16` in
/// `[1, 65535]`.
///
/// We compute this via a binary search over [`STRETCH`] (which is monotonic
/// increasing). The cost is ~12 comparisons per call — negligible compared
/// to the arithmetic coder.
#[inline]
fn squash(s: i32) -> u16 {
    // Clamp the input to the range covered by the stretch table.
    let s_lo = STRETCH[1];
    let s_hi = STRETCH[STRETCH_SIZE - 1];
    let s_clamped = s.clamp(s_lo, s_hi);

    // Binary search: find the largest index `idx` such that
    // STRETCH[idx] <= s_clamped. The corresponding probability bucket is
    // `idx`, which we map back to the u16 range.
    let mut lo = 1usize;
    let mut hi = STRETCH_SIZE - 1;
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        if STRETCH[mid] <= s_clamped {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    // Convert 12-bit bucket back to 16-bit probability: idx * 16 (with
    // midpoint rounding). idx is in [1, 4095].
    let prob = (lo as i32) << 4;
    prob.clamp(PROB_MIN, PROB_MAX) as u16
}

// ---------------------------------------------------------------------------
// Mixer
// ---------------------------------------------------------------------------

/// Adaptive logistic mixer combining [`NUM_MODELS`] model predictions.
///
/// Owned and deterministic: the same sequence of `(probs, bit)` updates
/// produces identical internal state regardless of platform or run order.
#[derive(Debug, Clone)]
pub struct Mixer {
    /// Weights in Q.8 fixed-point signed. Initialised to equal weighting
    /// (`WEIGHT_SCALE / NUM_MODELS` each).
    weights: [i32; NUM_MODELS],
    /// Most recent stretched predictions, kept for the post-bit update.
    last_stretched: [i32; NUM_MODELS],
    /// Most recent mixed probability (for the update step).
    last_prob: u16,
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

impl Mixer {
    /// Construct a new mixer with initial weights that make it behave as a
    /// pure pass-through of the order-2 model (index 2). The other models
    /// start with zero weight and can be boosted by adaptation when they
    /// consistently add information.
    ///
    /// Rationale: on short inputs (kilobytes), the order-2 byte-context
    /// model is the strongest single predictor. Starting the others at zero
    /// means Phase 2 never does *worse* than Phase 1 — it can only improve
    /// as adaptation discovers useful contributions from order-0/1/3/match/run.
    #[must_use]
    pub fn new() -> Self {
        // Indices: 0=order-0, 1=order-1, 2=order-2, 3=order-3, 4=match, 5=run-length.
        let weights = [0, 0, WEIGHT_SCALE, 0, 0, 0];
        Self {
            weights,
            last_stretched: [0; NUM_MODELS],
            last_prob: 1 << 15, // uniform
        }
    }

    /// Combine the model probabilities into one probability for the coder.
    ///
    /// `probs[i]` is `P(bit=1) * 65536` from model `i`, in `[1, 65535]`.
    #[must_use]
    pub fn mix(&mut self, probs: &[u16; NUM_MODELS]) -> u16 {
        let mut sum: i32 = 0;
        for (i, &p) in probs.iter().enumerate() {
            let s = stretch(p);
            self.last_stretched[i] = s;
            // sum += w_i * s_i, where w is Q.8 and s is Q.8 -> result is Q.16
            // but we'll re-normalise below.
            sum += self.weights[i] * s;
        }
        // `sum` is sum_i w_i * s_i in Q.16. squash expects Q.8, so shift back.
        // We don't divide by sum(weights) — logistic mixing does not
        // normalise: the weights act as gain controls, and adaptation finds
        // the right magnitudes.
        let s8 = sum >> WEIGHT_FRAC_BITS;
        let p = squash(s8);
        self.last_prob = p;
        p
    }

    /// Adapt weights after the true `bit` is known.
    ///
    /// Logistic SGD step: `w_i += lr * (bit - p) * s_i`, where `p` is the
    /// mixed probability (as a fraction in `[0,1]`) and `s_i` is the
    /// stretched prediction from model `i`.
    pub fn update(&mut self, bit: bool) {
        // err = bit - p, scaled to fixed-point Q.16. bit is 0 or 1, so:
        //   err_q16 = bit * 65536 - last_prob
        let err_q16: i32 = if bit {
            PROB_SCALE - i32::from(self.last_prob)
        } else {
            -i32::from(self.last_prob)
        };

        // lr = LEARN_NUM / LEARN_DEN (in raw units). The full update is:
        //   w_i += lr * err * s_i
        // We compute it as:
        //   w_i += (err_q16 * s_i) / (LEARN_DEN * 2^16 / LEARN_NUM)
        // To avoid rounding bias and keep things simple, accumulate in i64.
        // The numerator err_q16 * s_i has magnitude up to 65536 * 1468 ~ 1e8,
        // comfortably within i32, but the intermediate /scaling/ could push
        // things around so we use i64 to be safe.
        let denom = (i64::from(LEARN_DEN) * (1i64 << 16)) / i64::from(LEARN_NUM);
        for i in 0..NUM_MODELS {
            let prod = i64::from(err_q16) * i64::from(self.last_stretched[i]);
            let delta = (prod / denom) as i32;
            // Update with clamping to a sane range to prevent runaway.
            let new_w = self.weights[i].saturating_add(delta);
            self.weights[i] = new_w.clamp(-32 * WEIGHT_SCALE, 32 * WEIGHT_SCALE);
        }
    }

    /// Read-only access to the current weights (for testing / inspection).
    #[must_use]
    pub fn weights(&self) -> [i32; NUM_MODELS] {
        self.weights
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
mod tests {
    use super::*;

    #[test]
    fn stretch_squash_round_trip_error_bounded() {
        // Measure the round-trip error of stretch -> squash for every
        // probability bucket. The 12-bit stretch table introduces
        // quantization error from (a) 4096-bucket probability quantisation,
        // (b) fixed-point logit series approximation, and (c) binary-search
        // squash resolution. We verify the error stays below 128 (< 0.2%
        // probability), which is negligible for the arithmetic coder.
        let mut max_err = 0i32;
        let mut max_err_at = 0u16;
        for bucket in 1..STRETCH_SIZE {
            let prob_u16 = ((bucket as i32) << 4).clamp(1, PROB_MAX) as u16;
            let s = stretch(prob_u16);
            let recovered = i32::from(squash(s));
            let err = (recovered - i32::from(prob_u16)).abs();
            if err > max_err {
                max_err = err;
                max_err_at = prob_u16;
            }
        }
        assert!(
            max_err <= 128,
            "stretch/squash round-trip error {max_err} (at p={max_err_at}) exceeds 128"
        );
    }

    #[test]
    fn stretch_table_is_well_formed() {
        // The stretch table must be monotonic non-decreasing with the
        // expected signs at the extremes and ~0 at the midpoint.
        assert!(STRETCH[1] < 0, "near-zero prob should give negative logit");
        assert!(
            STRETCH[STRETCH_SIZE - 1] > 0,
            "near-one prob should give positive logit"
        );
        for i in 1..STRETCH_SIZE - 1 {
            assert!(STRETCH[i] <= STRETCH[i + 1], "stretch not monotonic at {i}");
        }
        let uniform = stretch(1 << 15);
        assert!(
            uniform.abs() < 50,
            "uniform stretch should be ~0, got {uniform}"
        );
    }

    #[test]
    fn mix_uniform_inputs_gives_uniform_output() {
        let mut m = Mixer::new();
        let p = m.mix(&[1 << 15; NUM_MODELS]);
        let diff = (i32::from(p) - (1 << 15)).abs();
        assert!(diff < 100, "uniform mix should be ~uniform, got {p}");
    }

    #[test]
    fn mix_extreme_high_input_gives_high_output() {
        let mut m = Mixer::new();
        let p = m.mix(&[65_000; NUM_MODELS]);
        assert!(p > 60_000, "all-high inputs should give high mix, got {p}");
    }

    #[test]
    fn mix_extreme_low_input_gives_low_output() {
        let mut m = Mixer::new();
        let p = m.mix(&[10; NUM_MODELS]);
        assert!(p < 1000, "all-low inputs should give low mix, got {p}");
    }

    #[test]
    fn update_increases_weight_for_correct_model() {
        // Model 0 always predicts high (prob 65000), others predict low.
        // The bit is 1, so model 0 should gain weight; others should lose.
        let mut m = Mixer::new();
        let initial = m.weights();
        for _ in 0..200 {
            let _ = m.mix(&[65_000, 100, 100, 100, 100, 100]);
            m.update(true);
        }
        let after = m.weights();
        assert!(
            after[0] > initial[0],
            "correct model weight should increase: {} -> {}",
            initial[0],
            after[0]
        );
        assert!(
            after[1] < initial[1],
            "wrong model weight should decrease: {} -> {}",
            initial[1],
            after[1]
        );
    }

    #[test]
    fn mixer_is_deterministic() {
        let run = || {
            let mut m = Mixer::new();
            let mut out = Vec::new();
            for i in 0..500u32 {
                let probs = [
                    (i % 65_535) as u16 + 1,
                    ((i * 7) % 65_535) as u16 + 1,
                    ((i * 13) % 65_535) as u16 + 1,
                    ((i * 31) % 65_535) as u16 + 1,
                    ((i * 47) % 65_535) as u16 + 1,
                    ((i * 53) % 65_535) as u16 + 1,
                ];
                let p = m.mix(&probs);
                m.update(p > 32_000);
                out.push((m.weights(), p));
            }
            out
        };
        let a = run();
        let b = run();
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(x, y, "non-deterministic mixer state at step {i}");
        }
    }

    /// When one model is consistently correct and the others consistently
    /// wrong, the mixer's output should converge toward the correct model.
    #[test]
    fn mixer_converges_to_correct_model() {
        let mut m = Mixer::new();
        // Model 0 predicts ~1, others ~0; the true bit alternates such that
        // model 0 is right more often.
        let mut correct = 0usize;
        let mut total = 0usize;
        for i in 0..2000u32 {
            let bit = i % 3 != 0; // bit=1 with prob 2/3
            let probs = if bit {
                [60_000u16, 5_000, 5_000, 5_000, 5_000, 5_000]
            } else {
                [5_000u16, 60_000, 60_000, 60_000, 60_000, 60_000]
            };
            let p = m.mix(&probs);
            // "Correct" = p > 0.5 when bit=1, p < 0.5 when bit=0.
            let pred_one = p > 32_000;
            if pred_one == bit {
                correct += 1;
            }
            total += 1;
            m.update(bit);
        }
        let acc = correct as f64 / total as f64;
        assert!(acc > 0.6, "mixer accuracy {acc:.3} should exceed 0.6");
    }
}
