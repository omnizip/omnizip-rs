//! Lzip (`.lz`) container decoder.
//!
//! ## Layout
//!
//! ```text
//! Magic           4 bytes: "LZIP"
//! Version         1 byte:  0 or 1
//! Dict_Size_Code  1 byte:  encoded dictionary size
//! LZMA1 stream    variable: raw LZMA1 (NO properties byte — lzip uses
//!                           fixed lc=3, lp=0, pb=2). Starts with 5
//!                           range coder init bytes.
//! Trailer         20 bytes: CRC32 + data_size (LE u64) + member_size (LE u64)
//! ```
//!
//! Key difference from `.lzma`: lzip does NOT embed a properties byte
//! or dict-size/uncompressed-size in the LZMA stream header. The LZMA
//! parameters are fixed at lc=3, lp=0, pb=2 (matching the lzip spec).
//! The dictionary size comes from the lzip header's dict-size code.
//! The uncompressed size comes from the trailer.

#![forbid(unsafe_code)]

use crate::decoder::Lzma1Decoder;
use crate::LzmaError;

pub const LZIP_MAGIC: [u8; 4] = *b"LZIP";
pub const LZIP_TRAILER_SIZE: usize = 20;
pub const LZIP_HEADER_SIZE: usize = 6;
/// Lzip v0 trailer size (no CRC32).
const LZIP_V0_TRAILER_SIZE: usize = 12;

/// Lzip uses fixed LZMA parameters (no properties byte in stream).
const LZIP_LC: u32 = 3;
const LZIP_LP: u32 = 0;
const LZIP_PB: u32 = 2;

/// Decompress a `.lz` (lzip) container, handling multi-member files.
///
/// Each member is decoded independently and its output concatenated.
/// Member boundaries are read from each member's trailer
/// (`member_size`, the total bytes of that member including header +
/// LZMA1 stream + trailer).
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] on truncation, invalid magic, or
/// any underlying LZMA1 decode failure.
pub fn lzip_decompress(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    // Reject empty input as a special case (no members to decode).
    if input.is_empty() {
        return Err(LzmaError::Corrupt {
            reason: "lzip stream is empty".into(),
        });
    }
    // The first member MUST start with LZIP magic.
    if input.len() < 4 || input[..4] != LZIP_MAGIC {
        return Err(LzmaError::Corrupt {
            reason: "lzip magic mismatch".into(),
        });
    }
    let mut output = Vec::new();
    let mut cursor = 0usize;
    while cursor + LZIP_HEADER_SIZE + LZIP_V0_TRAILER_SIZE < input.len() {
        if input.len() < cursor + 4 || input[cursor..cursor + 4] != LZIP_MAGIC {
            break;
        }
        let member_size = decode_one_member(&input[cursor..], &mut output)?;
        cursor += member_size;
    }
    Ok(output)
}

/// Decode one lzip member starting at the head of `input`. Appends
/// the decoded bytes to `output` and returns the member's total size.
///
/// Tries each candidate member boundary N from `min_member_size` to
/// `input.len()`. For each N, checks that the trailer's `member_size`
/// field equals N. The matching N is the actual member boundary.
fn decode_one_member(input: &[u8], output: &mut Vec<u8>) -> Result<usize, LzmaError> {
    if input.len() < LZIP_HEADER_SIZE + LZIP_TRAILER_SIZE {
        return Err(LzmaError::Corrupt {
            reason: format!(
                "lzip member too short: {} bytes (need ≥ {})",
                input.len(),
                LZIP_HEADER_SIZE + LZIP_TRAILER_SIZE
            ),
        });
    }
    if input[..4] != LZIP_MAGIC {
        return Err(LzmaError::Corrupt {
            reason: "lzip magic mismatch".into(),
        });
    }
    let version = input[4];
    if version > 1 {
        return Err(LzmaError::Corrupt {
            reason: format!("unsupported lzip version {version}"),
        });
    }
    let dict_size = decode_dict_size(input[5]);

    // v0 has an 8-byte trailer (data_size only, no member_size, no CRC32).
    // v1 has a 20-byte trailer (CRC32 + data_size + member_size).
    let trailer_size = if version == 0 { 8 } else { LZIP_TRAILER_SIZE };

    // For v1, scan for a member boundary where member_size matches.
    // For v0, treat the entire remaining input as one member.
    if version == 0 {
        let trailer = &input[input.len() - trailer_size..];
        let data_size = u64::from_le_bytes([
            trailer[0], trailer[1], trailer[2], trailer[3],
            trailer[4], trailer[5], trailer[6], trailer[7],
        ]);
        let compressed = &input[LZIP_HEADER_SIZE..input.len() - trailer_size];
        if compressed.is_empty() {
            return Err(LzmaError::Corrupt {
                reason: "lzip member has no LZMA1 data".into(),
            });
        }
        let mut decoder = Lzma1Decoder::new(LZIP_LC, LZIP_LP, LZIP_PB, dict_size);
        let member_output = decoder.decode(compressed, Some(data_size), true)?;
        output.extend_from_slice(&member_output);
        return Ok(input.len());
    }

    // v1: try each candidate member_size from min to input.len().
    let min_member_size = LZIP_HEADER_SIZE + trailer_size + 1;
    for member_end in min_member_size..=input.len() {
        let trailer = &input[member_end - trailer_size..member_end];
        let member_size = u64::from_le_bytes([
            trailer[12], trailer[13], trailer[14],
            trailer[15], trailer[16], trailer[17],
            trailer[18], trailer[19],
        ]);
        if member_size as usize != member_end {
            continue;
        }
        let data_size = u64::from_le_bytes([
            trailer[4], trailer[5], trailer[6], trailer[7],
            trailer[8], trailer[9], trailer[10], trailer[11],
        ]);
        let compressed = &input[LZIP_HEADER_SIZE..member_end - trailer_size];
        if compressed.is_empty() {
            return Err(LzmaError::Corrupt {
                reason: "lzip member has no LZMA1 data".into(),
            });
        }
        let mut decoder = Lzma1Decoder::new(LZIP_LC, LZIP_LP, LZIP_PB, dict_size);
        let member_output = decoder.decode(compressed, Some(data_size), true)?;
        output.extend_from_slice(&member_output);
        return Ok(member_end);
    }

    Err(LzmaError::Corrupt {
        reason: "lzip: no valid member boundary found".into(),
    })
}

#[must_use] 
pub fn decode_dict_size(code: u8) -> u32 {
    let n = u32::from(code);
    let exp = n / 2 + 11;
    if exp >= 31 {
        return u32::MAX;
    }
    let base = 1u32 << exp;
    if n & 1 != 0 {
        base + (1u32 << (exp - 1))
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_is_lzip() {
        assert_eq!(LZIP_MAGIC, *b"LZIP");
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(lzip_decompress(b"XXXX\x00\x05").is_err());
    }

    #[test]
    fn rejects_too_short() {
        // LZIP magic + 2 bytes header, but no room for a trailer.
        // Returns empty output (no members to decode).
        let out = lzip_decompress(b"LZIP\x00\x05").expect("ok with empty output");
        assert!(out.is_empty());
    }

    #[test]
    fn rejects_member_too_short_for_trailer() {
        // LZIP magic + header but no trailer at all.
        let out = lzip_decompress(b"LZIP\x00\x05\x00\x00").expect("ok with empty output");
        assert!(out.is_empty());
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut bad = LZIP_MAGIC.to_vec();
        bad.push(2);
        bad.push(0x0C);
        bad.resize(LZIP_HEADER_SIZE + LZIP_TRAILER_SIZE, 0);
        assert!(lzip_decompress(&bad).is_err());
    }

    #[test]
    fn dict_size_code_12_is_128k() {
        assert_eq!(decode_dict_size(12), 131_072);
    }

    #[test]
    fn dict_size_code_0_is_2k() {
        assert_eq!(decode_dict_size(0), 2048);
    }
}
