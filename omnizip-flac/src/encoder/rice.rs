//! Partitioned Rice residual encoder.
//!
//! Residuals (after FIXED or LPC prediction) are stored using
//! partitioned Rice coding: the array is split into `1 << order`
//! partitions, each with its own Rice parameter `k`.
//!
//! Wire format (must match `rice::decode_residual`):
//! ```text
//! coding_method (2 bits)  = 0 (RICE, 4-bit parameter)
//! partition_order (4 bits)
//! for each partition:
//!   rice_parameter (4 bits, 0..=14, or 15 = escape)
//!   if escape:
//!     escape_bps (5 bits, raw storage bit depth)
//!     raw_residuals (n × escape_bps bits, signed)
//!   else:
//!     for each residual:
//!       unary(q) + binary(r, k)
//! ```
//! where `q` is encoded as `q` one-bits followed by a zero-bit
//! (matching the FLAC convention used by libFLAC).

#![forbid(unsafe_code)]

use crate::encoder::bitwriter::BitWriter;

/// Coding method 0: RICE with 4-bit parameters.
const RICE_METHOD: u64 = 0;

/// Escape parameter value for RICE (4-bit): signals raw storage.
const RICE_ESCAPE: u64 = 0b1111;

/// Maximum partition order tried by [`best_partition_order`].
///
/// libFLAC's default is 6 (== 64 partitions). Higher orders rarely
/// help on real audio and increase per-partition k-header overhead.
pub const MAX_PARTITION_ORDER: u8 = 6;

/// Pick the partition order (0..=[`MAX_PARTITION_ORDER`]) that yields
/// the smallest encoded bit count for `residuals`.
///
/// `block_size` is the subframe's block size in samples
/// (= `residuals.len() + predictor_order`); `predictor_order` is the
/// predictor order (FIXED or LPC). The cost estimation uses the libFLAC
/// partition layout (predictor_order subtraction in partition 0).
///
/// Returns `(order, total_bits_including_header)`.
#[must_use]
pub fn best_partition_order(
    residuals: &[i32],
    block_size: usize,
    predictor_order: u32,
) -> (u8, u64) {
    let n = residuals.len();
    if n == 0 {
        return (0, 10);
    }
    let mut best_order = 0u8;
    let mut best_bits = u64::MAX;
    for order in 0..=MAX_PARTITION_ORDER {
        let n_parts = 1usize << order;
        if n_parts > block_size {
            break;
        }
        let samples_per_partition = (block_size + n_parts - 1) / n_parts;
        // libFLAC rejects layouts where partition 0 would have zero
        // or negative residuals (samples_per_partition <
        // predictor_order). Skip those orders entirely.
        if samples_per_partition < predictor_order as usize {
            break;
        }
        let bits = encoded_bits_for_order(residuals, block_size, order, predictor_order);
        if bits < best_bits {
            best_bits = bits;
            best_order = order;
        }
    }
    (best_order, best_bits)
}

/// Compute the encoded bit count for `residuals` at a fixed `partition_order`.
///
/// Includes the 10-bit residual-section header. Uses the libFLAC
/// partition layout: partition 0 has
/// `(block_size >> partition_order) - predictor_order` residuals
/// (the `predictor_order` is subtracted from partition 0 only);
/// later partitions each have `block_size >> partition_order`
/// residuals.
fn encoded_bits_for_order(
    residuals: &[i32],
    block_size: usize,
    partition_order: u8,
    predictor_order: u32,
) -> u64 {
    let n_parts = 1usize << partition_order;
    let samples_per_partition = (block_size + n_parts - 1) / n_parts;

    let mut total: u64 = 10; // method (2) + partition_order (4) + reserved
    let mut offset = 0usize;
    for part_idx in 0..n_parts {
        let part_size = if part_idx == 0 {
            samples_per_partition.saturating_sub(predictor_order as usize)
        } else {
            samples_per_partition
        };
        let part_end = (offset + part_size).min(residuals.len());
        let part = &residuals[offset..part_end];
        offset = part_end;
        if part.is_empty() {
            continue;
        }
        let k = best_rice_parameter(part);
        total += 4; // k parameter field
        if k >= 15 {
            // Escape: 5-bit bps field + bps bits per residual.
            total += 5;
            total += 32 * part.len() as u64;
        } else {
            for &r in part {
                let mapped = map_to_unsigned(r);
                let q = mapped >> k;
                total += u64::from(q) + 1 + u64::from(k);
            }
        }
    }
    total
}

/// Encode a partitioned Rice residual block.
///
/// `residuals` is the full residual array (across all partitions).
/// `block_size` is the subframe's block size in samples
/// (= `residuals.len() + predictor_order`). The decoder computes
/// `partition_samples = block_size >> partition_order` and reads
/// `partition_samples - predictor_order` residuals from partition 0,
/// then `partition_samples` from each subsequent partition.
/// `predictor_order` is the subframe's predictor order.
/// `bps` is the sample bit depth — used for the escape code path.
///
/// # Errors
///
/// Returns `String` on invalid parameter combinations.
pub fn encode_residuals(
    writer: &mut BitWriter,
    residuals: &[i32],
    block_size: usize,
    predictor_order: u32,
    partition_order: u8,
    bps: u8,
) -> Result<(), String> {
    let n_parts = 1usize << partition_order;
    if residuals.is_empty() {
        return Err("residuals must be non-empty".into());
    }
    if n_parts > block_size {
        return Err(format!(
            "partition count {n_parts} exceeds block size {block_size}"
        ));
    }

    // Write 2-bit coding method + 4-bit partition order.
    writer.write_bits(RICE_METHOD, 2);
    writer.write_bits(u64::from(partition_order), 4);

    // Sample count per partition. This is `block_size >> partition_order`,
    // NOT `residuals.len() >> partition_order`. The libFLAC spec subtracts
    // `predictor_order` from partition 0's residual count separately,
    // so the partition sizing is based on the full block size.
    let samples_per_partition = (block_size + n_parts - 1) / n_parts;
    let mut offset = 0usize;
    for part_idx in 0..n_parts {
        let part_size = if part_idx == 0 {
            samples_per_partition.saturating_sub(predictor_order as usize)
        } else {
            samples_per_partition
        };
        // The last partition may need fewer residuals if there aren't
        // enough left.
        let part_end = (offset + part_size).min(residuals.len());
        let part = &residuals[offset..part_end];
        offset = part_end;

        let k = best_rice_parameter(part);
        if k >= 15 {
            // Escape: 4-bit param = 15, 5-bit bps, then raw signed values.
            writer.write_bits(RICE_ESCAPE, 4);
            writer.write_bits(u64::from(bps), 5);
            for &r in part {
                writer.write_signed(i64::from(r), bps);
            }
        } else {
            writer.write_bits(u64::from(k), 4);
            for &r in part {
                write_rice_quotient(writer, r, k);
            }
        }
    }

    Ok(())
}

/// Convenience: pick the best partition order and encode in one call.
///
/// Used by FIXED and LPC subframe encoders that don't have a reason
/// to force a specific partition order. Returns the order that was
/// used, for cost-estimation by the caller.
///
/// # Errors
///
/// See [`encode_residuals`].
pub fn encode_residuals_best(
    writer: &mut BitWriter,
    residuals: &[i32],
    block_size: usize,
    predictor_order: u32,
    bps: u8,
) -> Result<u8, String> {
    let (order, _) = best_partition_order(residuals, block_size, predictor_order);
    encode_residuals(writer, residuals, block_size, predictor_order, order, bps)?;
    Ok(order)
}

/// Choose the Rice parameter `k` (0..=14) that minimises encoded size.
/// Returns 15 (escape) when the partition is empty.
fn best_rice_parameter(partition: &[i32]) -> u8 {
    if partition.is_empty() {
        return 15;
    }

    // Mean of the mapped (unsigned) residual values. Under the
    // geometric-distribution assumption, the optimal k is:
    //   k* = floor(log2(mean / 2))
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

/// FLAC's signed-to-unsigned mapping:
///   r = 0  → 0
///   r > 0  → 2*r (even)
///   r < 0  → 2*|r| - 1 (odd)
fn map_to_unsigned(r: i32) -> u32 {
    ((r as u32) << 1) ^ ((r >> 31) as u32)
}

/// Write one Rice-coded residual: `unary(q) + binary(r_low, k)`.
fn write_rice_quotient(writer: &mut BitWriter, residual: i32, k: u8) {
    let mapped = map_to_unsigned(residual);
    let q = mapped >> k;
    let r_low = mapped & ((1u32 << k) - 1);
    writer.write_unary(q);
    if k > 0 {
        writer.write_bits(u64::from(r_low), k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitreader::BitReader;
    use crate::rice;

    #[test]
    fn map_to_unsigned_positive() {
        assert_eq!(map_to_unsigned(0), 0);
        assert_eq!(map_to_unsigned(1), 2);
        assert_eq!(map_to_unsigned(-1), 1);
        assert_eq!(map_to_unsigned(2), 4);
        assert_eq!(map_to_unsigned(-2), 3);
    }

    #[test]
    fn zero_residuals_uses_k0() {
        let mut w = BitWriter::new();
        let residuals = vec![0i32; 16];
        encode_residuals(&mut w, &residuals, 16, 0, 0, 16).expect("encode");
        let bytes = w.finish();
        // method=0 (2 bits), order=0 (4 bits), k=0 (4 bits) → first byte 0x00.
        // Each residual = 0 → unary(0) = just "0" (zero ones, then terminator 0).
        // Wait — unary(0) = 0 one-bits then 0-bit terminator = just "0".
        // So 16 residuals × 1 bit each = 16 bits = 2 bytes.
        // Total: 10 bits header + 16 bits data = 26 bits → 4 bytes (padded).
        assert!(bytes.len() <= 4);
        assert_eq!(bytes[0] & 0xFC, 0x00); // top 6 bits = method + order + k
    }

    #[test]
    fn large_residuals_choose_higher_k() {
        let part: Vec<i32> = (0..64).map(|i| i * 100).collect();
        let k = best_rice_parameter(&part);
        assert!(k > 0, "expected k > 0, got {k}");
    }

    #[test]
    fn escape_for_empty_partition() {
        let part: Vec<i32> = vec![];
        let k = best_rice_parameter(&part);
        assert_eq!(k, 15);
    }

    #[test]
    fn round_trip_via_decoder() {
        let residuals: Vec<i32> = vec![
            0, 1, -1, 2, -2, 3, -3, 100, -100, 0, 0, 0, 5, -5, 10, -10,
        ];
        let mut w = BitWriter::new();
        encode_residuals(&mut w, &residuals, 16, 0, 0, 16).expect("encode");
        w.flush_byte_aligned();
        let bytes = w.finish();

        let mut reader = BitReader::new(&bytes);
        let decoded = rice::decode_residual(&mut reader, residuals.len(), 0)
            .expect("decode");
        assert_eq!(decoded, residuals);
    }

    #[test]
    fn round_trip_multi_partition() {
        let residuals: Vec<i32> = (0..256).map(|i| (i - 128) * 3).collect();
        let mut w = BitWriter::new();
        encode_residuals(&mut w, &residuals, 256, 0, 2, 16).expect("encode");
        w.flush_byte_aligned();
        let bytes = w.finish();

        let mut reader = BitReader::new(&bytes);
        let decoded = rice::decode_residual(&mut reader, residuals.len(), 0)
            .expect("decode");
        assert_eq!(decoded, residuals);
    }
}
