//! Partitioned Rice residual decoder for FLAC.
//!
//! FLAC uses Rice coding for the prediction residual. The residual is
//! partitioned into 2^n groups, each with its own Rice parameter.

#![forbid(unsafe_code)]

use crate::bitreader::BitReader;

/// Decode the residual for a subframe.
///
/// Uses the libFLAC partition layout: partition 0 holds
/// `(block_size >> partition_order) - predictor_order` residuals;
/// later partitions each hold `block_size >> partition_order`
/// residuals. This matches what our encoder writes per the FLAC spec.
///
/// `block_size` is the subframe's block size in samples (= the
/// `block_size` argument to `decode_subframe`); `predictor_order`
/// is the subframe's predictor order.
///
/// # Errors
///
/// Returns `String` error on malformed residual data.
pub fn decode_residual(
    reader: &mut BitReader,
    block_size: usize,
    predictor_order: u32,
) -> Result<Vec<i32>, String> {
    if block_size == 0 {
        return Ok(Vec::new());
    }

    // Read coding method (2 bits): 0 = RICE, 1 = RICE2.
    let method = reader.read_bits(2);
    if method > 1 {
        return Err(format!("invalid residual coding method: {method}"));
    }

    let param_bits = if method == 0 { 4u32 } else { 5u32 };
    let escape_value = if method == 0 { 0x0F } else { 0x1F };

    // Read partition order (4 bits).
    let partition_order = reader.read_bits(4) as usize;
    let num_partitions = 1usize << partition_order;

    // Samples per partition (based on block_size, not residual_count).
    // Partition 0 has `samples_per_partition - predictor_order`
    // residuals; later partitions each have `samples_per_partition`.
    let samples_per_partition = (block_size + num_partitions - 1) / num_partitions;
    let mut residual = Vec::with_capacity(block_size);

    for part_idx in 0..num_partitions {
        let part_size = if part_idx == 0 {
            samples_per_partition.saturating_sub(predictor_order as usize)
        } else {
            samples_per_partition
        };
        let rice_param = reader.read_bits(param_bits);

        if rice_param == escape_value {
            // Escape: raw samples at bps bits each.
            let bps = reader.read_bits(5) as usize;
            for _ in 0..part_size {
                let raw = reader.read_bits(bps as u32);
                let val = if bps > 0 && raw & (1 << (bps - 1)) != 0 {
                    let mask = u32::MAX << bps;
                    (raw | mask) as i32
                } else {
                    raw as i32
                };
                residual.push(val);
            }
        } else {
            for _ in 0..part_size {
                residual.push(reader.read_rice_signed(rice_param));
            }
        }
    }

    Ok(residual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_empty_residual() {
        // Should handle gracefully.
        let mut reader = BitReader::new(&[]);
        let result = decode_residual(&mut reader, 0, 0);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
