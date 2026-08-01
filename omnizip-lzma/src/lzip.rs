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
#![warn(clippy::pedantic)]

use crate::decoder::Lzma1Decoder;
use crate::LzmaError;

pub const LZIP_MAGIC: [u8; 4] = *b"LZIP";
pub const LZIP_TRAILER_SIZE: usize = 20;
pub const LZIP_HEADER_SIZE: usize = 6;

/// Lzip uses fixed LZMA parameters (no properties byte in stream).
const LZIP_LC: u32 = 3;
const LZIP_LP: u32 = 0;
const LZIP_PB: u32 = 2;

pub fn lzip_decompress(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    if input.len() < LZIP_HEADER_SIZE + LZIP_TRAILER_SIZE {
        return Err(LzmaError::Corrupt {
            reason: format!(
                "lzip stream too short: {} bytes (need ≥ {})",
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

    let trailer = &input[input.len() - LZIP_TRAILER_SIZE..];
    let data_size = u64::from_le_bytes([
        trailer[4], trailer[5], trailer[6], trailer[7],
        trailer[8], trailer[9], trailer[10], trailer[11],
    ]);

    // LZMA1 stream: everything between header and trailer.
    // No properties byte — lzip uses fixed lc=3, lp=0, pb=2.
    let compressed = &input[LZIP_HEADER_SIZE..input.len() - LZIP_TRAILER_SIZE];
    if compressed.is_empty() {
        return Err(LzmaError::Corrupt {
            reason: "lzip member has no LZMA1 data".into(),
        });
    }

    let mut decoder = Lzma1Decoder::new(LZIP_LC, LZIP_LP, LZIP_PB, dict_size);
    decoder.decode(compressed, Some(data_size), true)
}

fn decode_dict_size(code: u8) -> u32 {
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
        assert!(lzip_decompress(b"LZIP\x00\x05").is_err());
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
