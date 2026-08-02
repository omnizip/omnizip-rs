//! Huffman table reader — extracts per-symbol weights from the ZSTD
//! wire format (RFC 8878 §4.2.1), then builds a [`HuffmanTable`].
//!
//! Verified against `HUF_readStats_body` in
//! `~/src/external/zstd/lib/common/entropy_common.c:243-306`.
//!
//! ## Weight encoding
//!
//! The first byte `iSize = bytes[0]` selects the encoding:
//!
//! - **`iSize >= 128`** — direct encoding. The next `((oSize + 1) / 2)`
//!   bytes pack `oSize = iSize - 127` weights, 4 bits each (high nibble
//!   first). The last weight is implied by the Kraft inequality (see
//!   [`implied_last_weight`]).
//! - **`iSize < 128`** — FSE-compressed weights (TODO: requires FSE
//!   table-from-stream reader; not yet ported).

#![forbid(unsafe_code)]

use crate::huffman::HuffmanTable;
use crate::ZstdError;

/// Maximum number of distinct Huffman symbols ZSTD supports (one per
/// possible byte value).
pub const HUF_SYMBOLVALUE_MAX: usize = 255;

/// Maximum Huffman code length (tableLog) ZSTD permits.
pub const HUF_TABLELOG_MAX: u8 = 11;

/// Read the Huffman weights from the head of `src` and construct a
/// [`HuffmanTable`]. Returns the table and the number of bytes consumed
/// from `src`.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on truncation, invalid weight values,
/// or Kraft-inequality violations.
pub fn read_huffman_table(src: &[u8]) -> Result<(HuffmanTable, usize), ZstdError> {
    if src.is_empty() {
        return Err(ZstdError::Corrupt {
            reason: "empty huffman header".into(),
        });
    }
    let i_size = usize::from(src[0]);

    let (weights, consumed) = if i_size >= 128 {
        read_direct_weights(src)?
    } else {
        read_fse_compressed_weights(src)?
    };

    let table = HuffmanTable::from_weights(&weights)?;
    Ok((table, consumed))
}

/// Direct (uncompressed) weights: byte 0 is `iSize = 127 + oSize`, the
/// next `(oSize + 1) / 2` bytes pack two 4-bit weights per byte.
fn read_direct_weights(src: &[u8]) -> Result<(Vec<u8>, usize), ZstdError> {
    let i_size = usize::from(src[0]);
    let o_size = i_size - 127;
    let packed_bytes = o_size.div_ceil(2);
    let needed = 1 + packed_bytes;
    if src.len() < needed {
        return Err(ZstdError::Corrupt {
            reason: format!(
                "truncated direct huffman weights: need {needed} bytes, got {}",
                src.len()
            ),
        });
    }
    let mut weights = Vec::with_capacity(o_size + 1);
    for n in (0..o_size).step_by(2) {
        let byte = src[1 + n / 2];
        weights.push(byte >> 4);
        if n + 1 < o_size {
            weights.push(byte & 0x0F);
        }
    }

    // The last weight is implied by Kraft: total of `1<<w >> 1` over
    // all weights must equal `1 << tableLog`, so the missing weight
    // fills the gap.
    let implied = implied_last_weight(&weights)?;
    weights.push(implied);

    Ok((weights, needed))
}

/// Compute the implied last weight from the weights seen so far, using
/// the Kraft inequality: `sum(1 << w >> 1) == 1 << tableLog`.
///
/// Matches the C reference:
/// ```text
/// tableLog    = highbit32(weightTotal) + 1
/// rest        = (1 << tableLog) - weightTotal
/// lastWeight  = highbit32(rest) + 1
/// ```
/// `rest` must be a clean power of two; otherwise the table is corrupt.
fn implied_last_weight(weights: &[u8]) -> Result<u8, ZstdError> {
    let mut weight_total: u32 = 0;
    for &w in weights {
        if w > HUF_TABLELOG_MAX {
            return Err(ZstdError::Corrupt {
                reason: format!("huffman weight {w} exceeds tableLog max"),
            });
        }
        weight_total += (1u32 << w) >> 1;
    }
    if weight_total == 0 {
        return Err(ZstdError::Corrupt {
            reason: "huffman weights sum to 0".into(),
        });
    }
    let table_log = 32 - weight_total.leading_zeros(); // highbit32(x) + 1
    if table_log > u32::from(HUF_TABLELOG_MAX) {
        return Err(ZstdError::Corrupt {
            reason: format!("huffman tableLog {table_log} exceeds max"),
        });
    }
    let total = 1u32 << table_log;
    let rest = total - weight_total;
    if rest == 0 {
        return Err(ZstdError::Corrupt {
            reason: "huffman weights already sum to total; no implied weight needed".into(),
        });
    }
    // rest must be a power of two.
    if rest & (rest - 1) != 0 {
        return Err(ZstdError::Corrupt {
            reason: format!("huffman implied weight remainder {rest} is not a power of two"),
        });
    }
    let last_weight = 32 - rest.leading_zeros(); // highbit32(rest) + 1
    if last_weight > u32::from(HUF_TABLELOG_MAX) {
        return Err(ZstdError::Corrupt {
            reason: format!("huffman implied weight {last_weight} exceeds max"),
        });
    }
    Ok(last_weight as u8)
}

fn read_fse_compressed_weights(src: &[u8]) -> Result<(Vec<u8>, usize), ZstdError> {
    if src.is_empty() {
        return Err(ZstdError::Corrupt { reason: "FSE weights: empty".into() });
    }
    let i_size = usize::from(src[0]);
    if 1 + i_size > src.len() {
        return Err(ZstdError::Corrupt {
            reason: format!("FSE weights: need {} bytes, got {}", 1 + i_size, src.len()),
        });
    }
    let ts = &src[1..1 + i_size];
    let (table, tb) = crate::fse::read_fse_table(ts)?;
    let bs = &ts[tb..];
    let mut weights = crate::fse::decode_stream(&table, bs, HUF_SYMBOLVALUE_MAX)?;
    let implied = implied_last_weight(&weights)?;
    weights.push(implied);
    Ok((weights, 1 + i_size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_header_is_corrupt() {
        assert!(read_huffman_table(&[]).is_err());
    }

    #[test]
    fn direct_weights_round_trip_two_symbols() {
        // Two present symbols with weights 1 and 1 → both length 1.
        // iSize = 127 + 2 = 129 = 0x81. packed = 1 byte: (1 << 4) | 1 = 0x11.
        // After adding implied weight: total = 1<<1>>1 + 1<<1>>1 = 1 + 1 = 2.
        // tableLog = highbit32(2) + 1 = 1 + 1 = 2. rest = 4 - 2 = 2.
        // lastWeight = highbit32(2) + 1 = 1 + 1 = 2. (But 2 > 1 = HUF_TABLELOG_MAX for 2 symbols?)
        // Actually HUF_TABLELOG_MAX is 11, so 2 is fine.
        // Hmm wait — for weights [1, 1], codeLengths = [maxW - 1 + 1, maxW - 1 + 1] = [1, 1].
        // Then implied weight adds another symbol at index 2 with weight 2.
        // But that's a 3rd symbol — the test isn't testing what I want.
        // Skip this for now and just verify the C formula directly.
        let src = [0x81, 0x11];
        let (table, consumed) = read_huffman_table(&src).expect("decode");
        assert_eq!(consumed, 2);
        assert_eq!(table.weights().len(), 3);
        assert_eq!(table.weights()[0], 1);
        assert_eq!(table.weights()[1], 1);
        // Implied weight should be 2 (highbit32(rest=2) + 1 = 2).
        assert_eq!(table.weights()[2], 2);
    }

    #[test]
    fn truncated_direct_weights_is_corrupt() {
        // iSize = 0x85 → oSize = 6, packed = 3 bytes, total = 4. Truncate.
        assert!(read_huffman_table(&[0x85, 0x11, 0x22]).is_err());
    }

    #[test]
    fn implied_weight_rejects_non_power_of_two() {
        // Weights [1, 2] → total = 1 + 2 = 3. tableLog = 2. rest = 4 - 3 = 1.
        // 1 is a power of two. lastWeight = 1.
        let w = implied_last_weight(&[1, 2]).unwrap();
        assert_eq!(w, 1);

        // Weights [1, 3] → total = 1 + 4 = 5. tableLog = 3. rest = 8 - 5 = 3.
        // 3 is NOT a power of two. Should error.
        assert!(implied_last_weight(&[1, 3]).is_err());
    }
}
