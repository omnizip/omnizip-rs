//! Partitioned Rice residual decoder for FLAC.
//!
//! FLAC uses Rice coding for the prediction residual. The residual is
//! partitioned into 2^n groups, each with its own Rice parameter.

#![forbid(unsafe_code)]

use crate::bitreader::BitReader;

/// Decode the residual for a subframe.
///
/// # Errors
///
/// Returns `String` error on malformed residual data.
pub fn decode_residual(reader: &mut BitReader, sample_count: usize) -> Result<Vec<i32>, String> {
    if sample_count == 0 {
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

    // Samples per partition (ceiling division).
    let samples_per_partition = (sample_count + num_partitions - 1) / num_partitions;
    let mut residual = Vec::with_capacity(sample_count);
    let mut remaining = sample_count;

    for _ in 0..num_partitions {
        let partition_samples = remaining.min(samples_per_partition);
        let rice_param = reader.read_bits(param_bits);

        if rice_param == escape_value {
            // Escape: raw samples at bps bits each.
            // Read the escape bps (5 bits).
            let bps = reader.read_bits(5) as usize;
            for _ in 0..partition_samples {
                let raw = reader.read_bits(bps as u32);
                let val = if bps > 0 && raw & (1 << (bps - 1)) != 0 {
                    // Sign extend.
                    let mask = u32::MAX << bps;
                    (raw | mask) as i32
                } else {
                    raw as i32
                };
                residual.push(val);
            }
        } else {
            // Rice-coded partition.
            for _ in 0..partition_samples {
                residual.push(reader.read_rice_signed(rice_param));
            }
        }

        remaining = remaining.saturating_sub(partition_samples);
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
        let result = decode_residual(&mut reader, 0);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
