//! Container framing for the Deflate64 codec.
//!
//! The Ruby reference serialises the Huffman tables as JSON with a fixed
//! header (`literal_size: u32 BE, distance_size: u32 BE`). We use a compact
//! binary form instead (no JSON dependency) but preserve the same shape so
//! the format is self-describing and deterministic.
//!
//! ```text
//! +-------------------+--------------------+-----------------+----------------+
//! | lit_table_bytes   | dist_table_bytes   | literal_table   | distance_table | bitstream |
//! | u32 BE            | u32 BE             | (variable)      | (variable)     | (variable) |
//! +-------------------+--------------------+-----------------+----------------+------------+
//! ```

#![allow(clippy::cast_possible_truncation)]

use crate::encoder::Encoded;
use crate::huffman::HuffTable;

/// Pack an [`Encoded`] into the on-the-wire container.
#[must_use]
pub fn pack(encoded: &Encoded) -> Vec<u8> {
    let lit_bytes = encoded.literal_table.serialize();
    let dist_bytes = encoded.distance_table.serialize();
    let mut out =
        Vec::with_capacity(8 + lit_bytes.len() + dist_bytes.len() + encoded.bitstream.len());
    out.extend_from_slice(&(lit_bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&(dist_bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&lit_bytes);
    out.extend_from_slice(&dist_bytes);
    out.extend_from_slice(&encoded.bitstream);
    out
}

/// Split a packed container back into its tables and bitstream.
///
/// # Errors
///
/// Returns a descriptive string if the buffer is truncated or has impossible
/// length fields.
pub fn unpack(buf: &[u8]) -> Result<(HuffTable, HuffTable, &[u8]), String> {
    if buf.len() < 8 {
        return Err("container too short for header".to_string());
    }
    let lit_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let dist_len = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    if 8 + lit_len > buf.len() {
        return Err("literal table length exceeds buffer".to_string());
    }
    let mut off = 8usize;
    let lit_table = HuffTable::deserialize(buf, &mut off)
        .ok_or_else(|| "literal table malformed".to_string())?;
    // `lit_len` is authoritative for framing; `off` should have advanced by
    // exactly `lit_len` bytes.
    let _ = lit_len;
    if off + dist_len > buf.len() {
        return Err("distance table length exceeds buffer".to_string());
    }
    let dist_table = HuffTable::deserialize(buf, &mut off)
        .ok_or_else(|| "distance table malformed".to_string())?;
    let _ = dist_len;
    let bitstream = &buf[off..];
    Ok((lit_table, dist_table, bitstream))
}
