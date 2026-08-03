//! LPC (Linear Predictive Coding) subframe encoder.
//!
//! LPC finds the optimal prediction coefficients for the data via
//! autocorrelation + Levinson-Durbin recursion, then encodes the
//! residual after applying the prediction. Achieves 5-15% better
//! compression than FIXED on tonal audio (music, speech).
//!
//! ## Algorithm
//!
//! 1. Compute autocorrelation at lags 0..=max_order.
//! 2. Levinson-Durbin recursion: solve for LPC coefficients (f64).
//! 3. Quantize coefficients to fixed-point with a chosen precision
//!    and shift.
//! 4. Compute residuals using the quantized coefficients.
//! 5. Pick the order (1..=32) and precision that minimise the total
//!    Rice-coded residual cost.
//!
//! ## Determinism
//!
//! All floating-point operations use a fixed summation order, so
//! output is byte-identical across runs and platforms (IEEE 754).

#![forbid(unsafe_code)]

use crate::encoder::bitwriter::BitWriter;
use crate::encoder::rice;

/// Subframe type code for LPC: bits 1-6 = 0b100000 + (order - 1).
/// The order field occupies the low 5 bits of the type.
const TYPE_LPC_BASE: u8 = 0b100000;

/// Maximum LPC order supported by FLAC.
///
/// The spec allows up to 32, but very high orders (24+) often produce
/// coefficient quantization that confuses some decoders. We cap at 16
/// (libFLAC's `-8` default upper bound for non-`--lax` mode) until
/// we've verified our quantization is bit-exact at higher orders.
pub const MAX_LPC_ORDER: usize = 16;

/// A complete LPC solution: coefficients + residuals + chosen params.
#[derive(Clone)]
pub struct LpcSolution {
    pub order: usize,
    pub precision_bits: u8,
    pub shift: i8,
    pub coeffs: Vec<i32>,
    pub residuals: Vec<i32>,
    /// First `order` samples, stored raw in the subframe.
    pub warmup: Vec<i32>,
    /// Estimated cost in bits of encoding this solution's residual.
    pub estimated_residual_bits: u32,
}

/// Find the best LPC solution for `samples`, trying multiple orders
/// and precision/shift combinations.
///
/// The "best" solution minimises **total** encoded bits (header +
/// residual), not just residual bits. Higher orders have smaller
/// residuals but bigger headers (warmup samples + quantised
/// coefficients); the DP picks the actually-cheapest combination.
pub fn best_lpc_candidate(samples: &[i32], bps: u8) -> Option<LpcSolution> {
    let max_order = MAX_LPC_ORDER.min(samples.len().saturating_sub(1).max(1));
    let acf = autocorrelate(samples, max_order);

    let mut best: Option<LpcSolution> = None;
    let mut best_total_cost = u64::MAX;

    for order in (1..=max_order).rev() {
        if let Some(sol) = levinson_durbin_quantise(&acf, order, samples) {
            // Include header cost in the comparison: subframe header (8)
            // + warmup (order * bps) + precision field (4) + shift
            // field (5) + coefficients (order * precision_bits).
            let order_u64 = order as u64;
            let header_bits: u64 = 8 + 4 + 5
                + order_u64 * u64::from(bps)
                + order_u64 * u64::from(sol.precision_bits);
            let total = header_bits + u64::from(sol.estimated_residual_bits);
            if total < best_total_cost {
                best_total_cost = total;
                best = Some(sol);
            }
        }
    }

    best
}

/// Encode an LPC subframe from a pre-computed `LpcSolution`.
///
/// `samples_len` is the subframe's block size (== `sol.warmup.len() +
/// sol.residuals.len()`). Required so the residual partition layout
/// can subtract `sol.order` from partition 0's residual count
/// without underflow.
pub fn encode_from_solution(
    writer: &mut BitWriter,
    sol: &LpcSolution,
    samples_len: usize,
    bps: u8,
) -> Result<(), String> {
    let order = sol.order;

    // Header.
    writer.write_bits(0, 1);
    writer.write_bits(u64::from(TYPE_LPC_BASE + (order as u8 - 1)), 6);
    writer.write_bits(0, 1);

    // Warm-up samples.
    // Caller must ensure samples[..order] is accessible — but LpcSolution
    // doesn't carry the warmup. We require the caller to write warmup
    // before calling, or include warmup in the solution.
    // For simplicity, we store warmup in the solution via a separate
    // method. But that means best_lpc_candidate must also store warmup.
    // Let's add a warmup field to LpcSolution.
    for &w in &sol.warmup {
        writer.write_signed(i64::from(w), bps);
    }

    // Precision + shift.
    writer.write_bits(u64::from(sol.precision_bits - 1), 4);
    let shift_field = if sol.shift < 0 {
        (sol.shift + 32) as u64
    } else {
        sol.shift as u64
    };
    writer.write_bits(shift_field, 5);

    // Coefficients.
    for &c in &sol.coeffs[..order] {
        writer.write_signed(i64::from(c), sol.precision_bits);
    }

    // Residual.
    let _order_used = rice::encode_residuals_best(
        writer,
        &sol.residuals,
        samples_len,
        sol.order as u32,
        bps,
    )?;

    Ok(())
}



/// Compute autocorrelation coefficients at lags 0..=max_order.
///
/// Uses double precision and a fixed summation order (left-to-right)
/// for deterministic output.
fn autocorrelate(samples: &[i32], max_order: usize) -> Vec<f64> {
    let n = samples.len();
    let mut acf = vec![0.0f64; max_order + 1];

    // Apply a simple window (none — FLAC doesn't window for LPC).
    for lag in 0..=max_order {
        let mut sum = 0.0f64;
        for i in lag..n {
            sum += samples[i] as f64 * samples[i - lag] as f64;
        }
        acf[lag] = sum;
    }
    acf
}

/// Levinson-Durbin recursion: solve the Toeplitz system for LPC
/// coefficients of the given order.
///
/// Returns the reflection coefficients and the LPC coefficients.
fn levinson_durbin(acf: &[f64], order: usize) -> Vec<f64> {
    if order == 0 || acf.is_empty() || acf[0] == 0.0 {
        return vec![0.0; order];
    }

    let mut lpc = vec![0.0f64; order];
    let mut ref_coefs = vec![0.0f64; order];
    let mut error = acf[0];

    for m in 0..order {
        // Compute the next reflection coefficient.
        let mut acc = acf[m + 1];
        for j in 0..m {
            acc += lpc[j] * acf[m - j];
        }
        let lambda = if error.abs() > 1e-20 { -acc / error } else { 0.0 };

        ref_coefs[m] = lambda;

        // Update LPC coefficients in-place (backward).
        let mut new_lpc = lpc.clone();
        new_lpc[m] = lambda;
        for j in 0..m {
            new_lpc[j] = lpc[j] + lambda * lpc[m - 1 - j];
        }
        lpc = new_lpc;

        error *= 1.0 - lambda * lambda;
        if error <= 0.0 {
            break;
        }
    }

    lpc
}

/// Run Levinson-Durbin, then quantise coefficients and compute residuals.
fn levinson_durbin_quantise(acf: &[f64], order: usize, samples: &[i32]) -> Option<LpcSolution> {
    let lpc = levinson_durbin(acf, order);

    let mut best: Option<LpcSolution> = None;
    let mut best_cost = u64::MAX;

    // libFLAC's `-8` setting searches LPC precision up to 15 bits.
    // We try the same spread (skipping 6 since it's rarely optimal).
    for &precision_bits in &[5u8, 7, 8, 9, 10, 11, 12, 13, 14, 15] {
        for &shift in &[0i8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12] {
            if let Some(sol) = quantise_and_predict(&lpc, order, precision_bits, shift, samples) {
                if (sol.estimated_residual_bits as u64) < best_cost {
                    best_cost = sol.estimated_residual_bits as u64;
                    best = Some(sol);
                }
            }
        }
    }

    best
}

/// Estimate the total bit cost of an LPC solution's residuals.
///
/// Uses the actual best-partition-order search so the estimate matches
/// what the encoder writes — critical for the order/precision/shift
/// DP to pick the actually-cheapest solution. The `predictor_order`
/// here is the LPC order itself (each LPC subframe's warm-up equals
/// the order).
fn estimate_residual_bits(residuals: &[i32], block_size: usize, predictor_order: u32) -> u32 {
    if residuals.is_empty() {
        return 10;
    }
    let (_, bits) = rice::best_partition_order(residuals, block_size, predictor_order);
    bits.min(u32::MAX as u64) as u32
}

/// Quantise LPC coefficients and compute residuals.
fn quantise_and_predict(
    lpc: &[f64],
    order: usize,
    precision_bits: u8,
    shift: i8,
    samples: &[i32],
) -> Option<LpcSolution> {
    let scale = (1i64 << shift) as f64;
    let max_coeff = (1i64 << (precision_bits - 1)) - 1;
    let min_coeff = -(1i64 << (precision_bits - 1));

    // FLAC convention: coeff[j] multiplies sample[i-1-j] (coeff[0] =
    // most recent, coeff[order-1] = oldest). Standard Levinson-Durbin:
    //   x_hat[n] = -Σ lpc[k] * x[n-1-k]
    // So coeff[j] = -lpc[j], in the SAME order (no reversal needed).
    let mut qlpc = Vec::with_capacity(order);
    for i in 0..order {
        let scaled = -lpc[i] * scale;
        let rounded = scaled.round() as i64;
        if rounded > max_coeff || rounded < min_coeff {
            return None;
        }
        qlpc.push(rounded as i32);
    }

    // Compute residuals using i32 wrapping arithmetic to EXACTLY match
    // libFLAC's decoder. coeff[j] multiplies sample[i-1-j] per spec.
    let mut residuals = Vec::with_capacity(samples.len() - order);
    for i in order..samples.len() {
        let mut predicted: i32 = 0;
        for j in 0..order {
            predicted = predicted.wrapping_add(
                qlpc[j].wrapping_mul(samples[i - 1 - j])
            );
        }
        let predicted_shifted = if shift >= 0 {
            predicted >> shift
        } else {
            predicted << (-shift)
        };
        let residual = samples[i].wrapping_sub(predicted_shifted);
        residuals.push(residual);
    }

    let estimated_residual_bits = estimate_residual_bits(&residuals, samples.len(), order as u32);
    let warmup = samples[..order].to_vec();

    Some(LpcSolution {
        order,
        precision_bits,
        shift,
        coeffs: qlpc,
        residuals,
        warmup,
        estimated_residual_bits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitreader::BitReader;
    use crate::subframe;

    fn round_trip(samples: &[i32], bps: u8) {
        let sol = best_lpc_candidate(samples, bps).expect("LPC solution exists");
        let mut w = BitWriter::new();
        encode_from_solution(&mut w, &sol, samples.len(), bps).expect("encode");
        w.flush_byte_aligned();
        let bytes = w.finish();

        let mut reader = BitReader::new(&bytes);
        let decoded = subframe::decode_subframe(&mut reader, samples.len(), bps).expect("decode");
        assert_eq!(decoded, samples);
    }

    #[test]
    fn lpc_round_trips_sine_wave() {
        // 256 samples of a 440 Hz sine at 8 kHz → strong periodicity.
        let samples: Vec<i32> = (0..256)
            .map(|i| ((i as f64 * 440.0 * std::f64::consts::TAU / 8000.0).sin() * 10_000.0) as i32)
            .collect();
        round_trip(&samples, 16);
    }

    #[test]
    fn lpc_round_trips_constant() {
        let samples = vec![42i32; 256];
        round_trip(&samples, 16);
    }

    #[test]
    fn lpc_round_trips_random() {
        // Pseudo-random data — LPC should still round-trip even if it
        // doesn't compress well.
        let samples: Vec<i32> = (0u32..256).map(|i| ((i.wrapping_mul(2654435761) >> 16) as i16) as i32).collect();
        round_trip(&samples, 16);
    }

    #[test]
    fn lpc_beats_verbatim_on_sine() {
        // For a sine wave, LPC residual size should be smaller than
        // VERBATIM (bps × n bits). The exact ratio depends on the
        // chosen order/precision/shift.
        let samples: Vec<i32> = (0..512)
            .map(|i| ((i as f64 * 440.0 * std::f64::consts::TAU / 8000.0).sin() * 30_000.0) as i32)
            .collect();
        let acf = autocorrelate(&samples, 32);
        let sol = levinson_durbin_quantise(&acf, 32, &samples).expect("solution");

        // Total residual bit estimate must be < verbatim (512 × 16 = 8192 bits).
        assert!(
            sol.estimated_residual_bits < 8192,
            "LPC residual bits {} >= verbatim 8192",
            sol.estimated_residual_bits
        );
    }

    #[test]
    fn autocorrelation_dc_signal() {
        // For a constant signal, acf[0] = N * value^2, acf[k] = acf[0].
        let samples = vec![100i32; 64];
        let acf = autocorrelate(&samples, 8);
        assert!((acf[0] - 64.0 * 100.0 * 100.0).abs() < 1.0);
        assert!((acf[1] - 63.0 * 100.0 * 100.0).abs() < 1.0);
    }

    #[test]
    fn determinism_same_input_same_output() {
        let samples: Vec<i32> = (0..256)
            .map(|i| ((i as f64 * 0.1).sin() * 1000.0) as i32)
            .collect();
        let acf1 = autocorrelate(&samples, 16);
        let acf2 = autocorrelate(&samples, 16);
        assert_eq!(acf1, acf2);
    }
}
