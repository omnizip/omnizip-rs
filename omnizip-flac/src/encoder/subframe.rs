//! FLAC subframe encoders.
//!
//! Each audio channel has one subframe per frame. The encoder chooses
//! the smallest representation among:
//! - **CONSTANT** (type 0): all samples identical. 1 byte header + bps bits.
//! - **VERBATIM** (type 1): raw samples. 1 byte header + n × bps bits.
//! - **FIXED** (type 8..=12): polynomial prediction + Rice residual.
//!
//! Each encoder produces output that `subframe::decode_subframe` can
//! decode. The [`SubframeEncoder`] trait (OCP) lets new subframe types
//! be added without touching the frame encoder.

#![forbid(unsafe_code)]

use crate::encoder::bitwriter::BitWriter;
use crate::encoder::rice;

/// Subframe type codes (encoded in bits 1..6 of the subframe header byte).
const TYPE_CONSTANT: u8 = 0b000000;
const TYPE_VERBATIM: u8 = 0b000001;
const TYPE_FIXED_BASE: u8 = 0b001000; // + order (0..4)

/// One channel's worth of audio samples, signed.
pub type Samples<'a> = &'a [i32];

/// Encode one subframe, choosing the cheapest representation.
///
/// Writes the subframe header + payload to `writer`. Does NOT pad to
/// byte boundary — the caller aligns after all channels' subframes.
///
/// # Errors
///
/// Returns `String` on internal errors (e.g. FIXED prediction overflow).
pub fn encode_subframe(
    writer: &mut BitWriter,
    samples: Samples<'_>,
    bps: u8,
) -> Result<(), String> {
    // No wasted bits per sample (1-bit flag = 0).
    // Header layout: 1 bit (0) + 6 bits (type) + 1 bit (wasted flag = 0).
    let candidate = choose_type(samples, bps);

    match candidate {
        SubframeType::Constant(value) => {
            write_header(writer, TYPE_CONSTANT);
            writer.write_signed(i64::from(value), bps);
        }
        SubframeType::Verbatim => {
            write_header(writer, TYPE_VERBATIM);
            for &s in samples {
                writer.write_signed(i64::from(s), bps);
            }
        }
        SubframeType::Fixed { order, residuals } => {
            write_header(writer, TYPE_FIXED_BASE + order);
            // Warm-up samples: first `order` samples stored raw.
            for &s in &samples[..order as usize] {
                writer.write_signed(i64::from(s), bps);
            }
            rice::encode_residuals_best(writer, &residuals, bps)?;
        }
        SubframeType::Lpc { solution } => {
            crate::encoder::lpc::encode_from_solution(writer, &solution, bps)?;
        }
    }

    Ok(())
}

/// Internal enum for the chosen subframe type. Kept private — callers
/// just call `encode_subframe` and the choice is automatic.
enum SubframeType {
    Constant(i32),
    Verbatim,
    Fixed { order: u8, residuals: Vec<i32> },
    Lpc { solution: crate::encoder::lpc::LpcSolution },
}

/// Choose the cheapest subframe type for `samples`. The cost metric is
/// the number of bits each representation would consume.
fn choose_type(samples: &[i32], bps: u8) -> SubframeType {
    // CONSTANT: 8-bit header + bps bits.
    let verbatim_cost = 8 + (samples.len() as u32 * u32::from(bps));
    let constant_cost = 8 + u32::from(bps);

    // Try CONSTANT first.
    if let Some(value) = try_constant(samples) {
        if constant_cost <= verbatim_cost {
            return SubframeType::Constant(value);
        }
    }

    // Compute the best FIXED candidate.
    let fixed = best_fixed(samples, bps);

    // Compute the best LPC candidate (if block size allows).
    // TEMPORARILY DISABLED: LPC encoding has a remaining interop bug
    // against libFLAC (LOST_SYNC during decode). Falling back to
    // FIXED-only until TODO 97 Phase 2B/3 isolates the issue.
    let lpc: Option<crate::encoder::lpc::LpcSolution> = if false {
        crate::encoder::lpc::best_lpc_candidate(samples, bps)
    } else {
        None
    };

    // Compare costs. LPC and FIXED both have warmup + header + residual.
    let fixed_cost = match &fixed {
        Some((order, residuals, cost)) => {
            let header = 8 + u32::from(*order) * u32::from(bps);
            Some((header + cost, SubframeType::Fixed { order: *order, residuals: residuals.clone() }))
        }
        None => None,
    };

    let lpc_cost = match lpc {
        Some(sol) => {
            let header = 8 + (sol.order as u32 * u32::from(bps)) + 4 + 5 + (sol.order as u32 * u32::from(sol.precision_bits));
            let cost = header + sol.estimated_residual_bits;
            Some((cost, SubframeType::Lpc { solution: sol }))
        }
        None => None,
    };

    // Pick the cheapest of FIXED / LPC / VERBATIM.
    let candidates = [fixed_cost, lpc_cost];
    let best = candidates
        .into_iter()
        .flatten()
        .min_by_key(|(cost, _)| *cost);

    match best {
        Some((cost, variant)) if cost < verbatim_cost => variant,
        _ => SubframeType::Verbatim,
    }
}

/// Compute the best FIXED candidate (order + residuals + bit cost).
fn best_fixed(samples: &[i32], _bps: u8) -> Option<(u8, Vec<i32>, u32)> {
    let mut best: Option<(u8, Vec<i32>, u32)> = None;
    for order in 0..=4u8 {
        if samples.len() < order as usize {
            break;
        }
        let residuals = compute_fixed_residuals(samples, order);
        let cost = fixed_cost(samples.len(), _bps, order, &residuals);
        match &best {
            None => best = Some((order, residuals, cost)),
            Some((_, _, prev)) if cost < *prev => {
                best = Some((order, residuals, cost));
            }
            _ => {}
        }
    }
    best
}

/// Return `Some(value)` if all samples equal `value`, else `None`.
fn try_constant(samples: &[i32]) -> Option<i32> {
    if samples.is_empty() {
        return None;
    }
    let first = samples[0];
    if samples.iter().all(|&s| s == first) {
        Some(first)
    } else {
        None
    }
}

/// FIXED predictor coefficients per order (from the FLAC spec).
///
/// prediction[i] = Σ coeff[k] * sample[i-1-k], for k = 0..order-1.
/// residual[i] = sample[i] - prediction[i].
const FIXED_COEFFS: [[i32; 4]; 5] = [
    [0, 0, 0, 0],          // order 0: residual = sample
    [1, 0, 0, 0],          // order 1: pred = sample[i-1]
    [2, -1, 0, 0],         // order 2: pred = 2*a - b
    [3, -3, 1, 0],         // order 3
    [4, -6, 4, -1],        // order 4
];

/// Compute FIXED-predictor residuals for `samples` at the given `order`.
/// Output length = `samples.len() - order`.
fn compute_fixed_residuals(samples: &[i32], order: u8) -> Vec<i32> {
    let order = order as usize;
    if order >= samples.len() {
        return Vec::new();
    }
    let coeffs = FIXED_COEFFS[order];
    let mut out = Vec::with_capacity(samples.len() - order);
    for i in order..samples.len() {
        let pred = (0..order)
            .map(|k| coeffs[k] * samples[i - 1 - k])
            .sum::<i32>();
        out.push(samples[i] - pred);
    }
    out
}

/// Estimate the bit cost of encoding `residuals` via partitioned Rice
/// coding. Uses the actual best-partition-order search so the estimate
/// matches what the encoder actually writes.
fn fixed_cost(_len: usize, _bps: u8, _order: u8, residuals: &[i32]) -> u32 {
    if residuals.is_empty() {
        return 10;
    }
    let (_, bits) = rice::best_partition_order(residuals);
    bits.min(u32::MAX as u64) as u32
}

fn best_k(partition: &[i32]) -> u8 {
    if partition.is_empty() {
        return 15;
    }
    let mut mapped_sum: u64 = 0;
    for &r in partition {
        mapped_sum += u64::from(map_to_unsigned(r));
    }
    let mean = mapped_sum / partition.len() as u64;
    if mean <= 1 {
        return 0;
    }
    let half_mean = mean / 2;
    let k = if half_mean == 0 { 0 } else { 63 - half_mean.leading_zeros() };
    k.clamp(0, 14) as u8
}

fn map_to_unsigned(r: i32) -> u32 {
    ((r as u32) << 1) ^ ((r >> 31) as u32)
}

/// Write the 8-bit subframe header: 1 bit (0) + 6 bits (type) + 1 bit
/// (wasted-bits-flag = 0).
fn write_header(writer: &mut BitWriter, type_code: u8) {
    // Byte layout: 0 | type[5:0] | 0
    writer.write_bits(0, 1);
    writer.write_bits(u64::from(type_code), 6);
    writer.write_bits(0, 1); // no wasted bits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitreader::BitReader;
    use crate::subframe;

    fn round_trip(samples: &[i32], bps: u8) {
        let mut w = BitWriter::new();
        encode_subframe(&mut w, samples, bps).expect("encode");
        w.flush_byte_aligned();
        let bytes = w.finish();

        let mut reader = BitReader::new(&bytes);
        let decoded = subframe::decode_subframe(&mut reader, samples.len(), bps)
            .expect("decode");
        assert_eq!(decoded, samples);
    }

    #[test]
    fn constant_subframe_round_trips() {
        let samples = vec![42i32; 100];
        round_trip(&samples, 16);
    }

    #[test]
    fn verbatim_subframe_round_trips() {
        // Non-constant samples → FIXED won't help for random data,
        // VERBATIM is likely chosen.
        let samples: Vec<i32> = (0..32).map(|i| i * 137 % 1000).collect();
        round_trip(&samples, 16);
    }

    #[test]
    fn fixed_order_0_round_trips() {
        // Small values → FIXED order 0 produces small Rice residuals.
        let samples: Vec<i32> = (0..64).map(|i| (i % 7) as i32).collect();
        round_trip(&samples, 16);
    }

    #[test]
    fn fixed_order_1_on_ramp() {
        // Linear ramp → FIXED order 1 gives near-zero residuals.
        let samples: Vec<i32> = (0..128).map(|i| (i * 2) as i32).collect();
        round_trip(&samples, 16);
    }

    #[test]
    fn fixed_order_2_on_quadratic() {
        // Quadratic sequence → FIXED order 2 gives small residuals.
        let samples: Vec<i32> = (0..128).map(|i| (i * i) as i32).collect();
        round_trip(&samples, 16);
    }

    #[test]
    fn empty_samples_round_trip() {
        // Degenerate case — should not panic.
        round_trip(&[], 16);
    }

    #[test]
    fn sine_wave_uses_fixed() {
        // Sine-like wave → FIXED order 1 should produce small residuals.
        let samples: Vec<i32> = (0..256)
            .map(|i| ((i as f64 * 0.1).sin() * 1000.0) as i32)
            .collect();
        round_trip(&samples, 16);
    }
}
