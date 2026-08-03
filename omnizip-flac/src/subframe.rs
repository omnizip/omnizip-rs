//! FLAC subframe decoders.
//!
//! Each audio channel has one subframe per frame. The subframe type
//! determines how the samples are encoded:
//! - `SUBFRAME_CONSTANT` (0): all samples are the same value.
//! - `SUBFRAME_VERBATIM` (1): raw samples, no compression.
//! - `SUBFRAME_FIXED` (2): fixed polynomial prediction + Rice residual.
//! - `SUBFRAME_LPC` (3): linear predictive coding + Rice residual.

#![forbid(unsafe_code)]

use crate::bitreader::BitReader;
use crate::rice;
use crate::subframe_type::{TYPE_CONSTANT as SUBFRAME_CONSTANT, TYPE_FIXED_BASE as SUBFRAME_FIXED,
    TYPE_LPC_BASE as SUBFRAME_LPC, TYPE_VERBATIM as SUBFRAME_VERBATIM};

/// Decode a single subframe. Returns the decoded samples as i32 values.
///
/// # Errors
///
/// Returns `String` error on malformed subframe data.
pub fn decode_subframe(
    reader: &mut BitReader,
    block_size: usize,
    bps: u8,
) -> Result<Vec<i32>, String> {
    // Read 1-bit zero padding.
    let _ = reader.read_bits(1);

    // Read subframe type (6 bits).
    let type_byte = reader.read_bits(6) as u8;

    // Check for wasted bits per sample.
    let has_wasted = reader.read_bits(1);
    let wasted_bits = if has_wasted != 0 {
        // Per FLAC spec: unary-coded value k where the encoding is
        // `k` ZERO-bits followed by a single ONE-bit terminator.
        // The wasted-bits-per-sample count is then k+1.
        let mut count = 0u32;
        while reader.read_bits(1) == 0 {
            count += 1;
        }
        count + 1
    } else {
        0
    };

    // Samples are stored at (bps - wasted_bits) bits each. After decoding,
    // we shift left by wasted_bits to recover the original bps-width value.
    let effective_bps = bps
        .checked_sub(wasted_bits as u8)
        .ok_or_else(|| format!("wasted_bits {wasted_bits} exceeds bps {bps}"))?;

    let mut samples = match type_byte {
        SUBFRAME_CONSTANT => decode_constant(reader, block_size, effective_bps)?,
        SUBFRAME_VERBATIM => decode_verbatim(reader, block_size, effective_bps)?,
        t if t >= SUBFRAME_FIXED && t < SUBFRAME_FIXED + 8 => {
            let order = (t - SUBFRAME_FIXED) as usize;
            decode_fixed(reader, block_size, effective_bps, order)?
        }
        t if t >= SUBFRAME_LPC => {
            let order = (t - SUBFRAME_LPC) as usize + 1;
            decode_lpc(reader, block_size, effective_bps, order)?
        }
        _ => return Err(format!("invalid subframe type: {type_byte}")),
    };

    // Apply wasted bits shift.
    if wasted_bits > 0 {
        for s in &mut samples {
            *s <<= wasted_bits;
        }
    }

    Ok(samples)
}

/// Decode a CONSTANT subframe: one value repeated.
fn decode_constant(reader: &mut BitReader, block_size: usize, bps: u8) -> Result<Vec<i32>, String> {
    let raw = reader.read_bits(u32::from(bps));
    let value = sign_extend(raw, bps);
    Ok(vec![value; block_size])
}

/// Decode a VERBATIM subframe: raw samples.
fn decode_verbatim(reader: &mut BitReader, block_size: usize, bps: u8) -> Result<Vec<i32>, String> {
    let mut samples = Vec::with_capacity(block_size);
    for _ in 0..block_size {
        let raw = reader.read_bits(u32::from(bps));
        samples.push(sign_extend(raw, bps));
    }
    Ok(samples)
}

/// Decode a FIXED subframe (orders 0-4).
fn decode_fixed(
    reader: &mut BitReader,
    block_size: usize,
    bps: u8,
    order: usize,
) -> Result<Vec<i32>, String> {
    if order > 4 {
        return Err(format!("invalid fixed order: {order}"));
    }
    if block_size <= order {
        return Err("block size too small for fixed order".into());
    }

    // Read warm-up samples.
    let mut samples = Vec::with_capacity(block_size);
    for _ in 0..order {
        let raw = reader.read_bits(u32::from(bps));
        samples.push(sign_extend(raw, bps));
    }

    // Read Rice-coded residual.
    let residual = rice::decode_residual(reader, block_size, order as u32)?;

    // Apply fixed prediction to reconstruct samples.
    // FIXED prediction coefficients (from FLAC spec):
    // Order 0: pred = 0
    // Order 1: pred = sample[i-1]
    // Order 2: pred = 2*sample[i-1] - sample[i-2]
    // Order 3: pred = 3*sample[i-1] - 3*sample[i-2] + sample[i-3]
    // Order 4: pred = 4*sample[i-1] - 6*sample[i-2] + 4*sample[i-3] - sample[i-4]
    for (i, &res) in residual.iter().enumerate() {
        let predicted = match order {
            0 => 0i64,
            1 => i64::from(samples[i]),
            2 => 2 * i64::from(samples[i + 1]) - i64::from(samples[i]),
            3 => {
                3 * i64::from(samples[i + 2])
                    - 3 * i64::from(samples[i + 1])
                    + i64::from(samples[i])
            }
            4 => {
                4 * i64::from(samples[i + 3])
                    - 6 * i64::from(samples[i + 2])
                    + 4 * i64::from(samples[i + 1])
                    - i64::from(samples[i])
            }
            _ => 0,
        };
        let reconstructed = (predicted + i64::from(res)) as i32;
        samples.push(reconstructed);
    }

    Ok(samples)
}

/// Decode an LPC subframe (orders 1-32).
fn decode_lpc(
    reader: &mut BitReader,
    block_size: usize,
    bps: u8,
    order: usize,
) -> Result<Vec<i32>, String> {
    if order == 0 || order > 32 {
        return Err(format!("invalid LPC order: {order}"));
    }
    if block_size <= order {
        return Err("block size too small for LPC order".into());
    }

    // Read warm-up samples.
    let mut samples = Vec::with_capacity(block_size);
    for _ in 0..order {
        let raw = reader.read_bits(u32::from(bps));
        samples.push(sign_extend(raw, bps));
    }

    // Read LPC precision (4 bits) and shift (5 bits, signed).
    let lpc_precision = reader.read_bits(4) as usize;
    let lpc_shift_raw = reader.read_bits(5) as i32;
    let lpc_shift = if lpc_shift_raw > 15 {
        lpc_shift_raw - 32
    } else {
        lpc_shift_raw
    };

    // Read LPC coefficients.
    let precision_bits = lpc_precision + 1;
    let mut coeffs = Vec::with_capacity(order);
    for _ in 0..order {
        let raw = reader.read_bits(precision_bits as u32);
        let coeff = sign_extend(raw, precision_bits as u8);
        coeffs.push(coeff);
    }

    // Read Rice-coded residual.
    let residual = rice::decode_residual(reader, block_size, order as u32)?;

    // Apply LPC prediction. Per FLAC spec: coeff[j] multiplies the
    // sample at position (current - 1 - j), i.e. coeff[0] = most recent.
    // Uses i32 wrapping arithmetic to match libFLAC's decoder exactly.
    for (i, &res) in residual.iter().enumerate() {
        let sample_idx = order + i;
        let mut predicted: i32 = 0;
        for (j, &coeff) in coeffs.iter().enumerate() {
            predicted = predicted.wrapping_add(coeff.wrapping_mul(samples[sample_idx - 1 - j]));
        }
        let predicted_shifted = if lpc_shift >= 0 {
            predicted >> lpc_shift
        } else {
            predicted << (-lpc_shift)
        };
        let reconstructed = predicted_shifted.wrapping_add(res);
        samples.push(reconstructed);
    }

    Ok(samples)
}

/// Sign-extend a `bits`-wide unsigned value to i32.
fn sign_extend(value: u32, bits: u8) -> i32 {
    if bits == 0 || bits >= 32 {
        return value as i32;
    }
    let sign_bit = 1u32 << (bits - 1);
    if value & sign_bit != 0 {
        // Negative: extend the sign.
        let mask = u32::MAX << bits;
        (value | mask) as i32
    } else {
        value as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_extend_positive() {
        assert_eq!(sign_extend(0b0101, 4), 5);
    }

    #[test]
    fn sign_extend_negative() {
        assert_eq!(sign_extend(0b1001, 4), -7);
    }

    #[test]
    fn sign_extend_zero_bits() {
        assert_eq!(sign_extend(0, 0), 0);
    }
}
