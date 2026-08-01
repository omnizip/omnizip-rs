//! `.lzma` (LZMA-Alone) container decoder.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/lzma/lzma_alone_decoder.rb`
//! (191 LOC, MIT, Ribose Inc.). The format is the legacy LZMA Utils
//! container, used by `.lzma` files predating `.xz`:
//!
//! ```text
//! offset  size  field
//! 0       1     properties byte: lc + 9*lp + 45*pb
//! 1       4     dictionary size, little-endian
//! 5       8     uncompressed size, little-endian (UINT64_MAX = unknown)
//! 13      …     LZMA1 stream (may end with EOPM if size is unknown)
//! ```
//!
//! The Ruby wraps `XzUtilsDecoder`; we drive [`crate::decoder::Lzma1Decoder`]
//! directly with `allow_eopm = true` (the `.lzma` format always permits
//! EOPM, even when the size is known).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::decoder::Lzma1Decoder;
use crate::LzmaError;

/// Header size: 1 prop byte + 4 dict-size bytes + 8 uncompressed-size bytes.
const HEADER_SIZE: usize = 13;

/// Magic value for "unknown uncompressed size" in the `.lzma` header.
const UNKNOWN_SIZE: u64 = u64::MAX;

/// Decompress a complete `.lzma` (LZMA-Alone) byte stream.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] on header truncation, invalid property
/// bytes, or any decoder-side corruption.
pub fn lzma_alone_decompress(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    if input.len() < HEADER_SIZE {
        return Err(LzmaError::Corrupt {
            reason: format!(
                ".lzma header is {} bytes; need at least {HEADER_SIZE}",
                input.len()
            ),
        });
    }

    let props = u32::from(input[0]);
    // XZ Utils: lc = props % 9; lp = (props / 9) % 5; pb = (props / 9) / 5.
    let lc = props % 9;
    let lp = (props / 9) % 5;
    let pb = (props / 9) / 5;

    let dict_size = u32::from_le_bytes([input[1], input[2], input[3], input[4]]);
    let raw_size = u64::from_le_bytes([
        input[5],
        input[6],
        input[7],
        input[8],
        input[9],
        input[10],
        input[11],
        input[12],
    ]);

    let uncompressed_size = if raw_size == UNKNOWN_SIZE {
        None
    } else {
        Some(raw_size)
    };

    let stream = &input[HEADER_SIZE..];
    let mut decoder = Lzma1Decoder::new(lc, lp, pb, dict_size);
    decoder.decode(stream, uncompressed_size, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_too_short_is_corrupt() {
        let err = lzma_alone_decompress(&[0u8; 5]).unwrap_err();
        assert!(matches!(err, LzmaError::Corrupt { .. }));
    }

    #[test]
    #[should_panic(expected = "lc + lp must be")]
    fn header_with_invalid_props_panics_in_decoder_constructor() {
        // props byte 21 decodes to lc=3, lp=2, pb=0 — invalid since
        // lc + lp = 5 > 4. The constructor of Lzma1Decoder panics with
        // an explicit assertion before decode starts.
        let mut hdr = vec![0u8; HEADER_SIZE];
        hdr[0] = 21; // lc=3, lp=2, pb=0 → lc+lp=5 > 4
        hdr.extend_from_slice(&[0u8; 8]);
        let _ = lzma_alone_decompress(&hdr);
    }

    #[test]
    fn header_constants_match_ruby() {
        assert_eq!(HEADER_SIZE, 13);
        assert_eq!(UNKNOWN_SIZE, u64::MAX);
    }
}
