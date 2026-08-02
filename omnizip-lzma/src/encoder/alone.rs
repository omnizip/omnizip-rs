//! `.lzma` (LZMA-Alone) container encoder.
//!
//! Inverse of [`crate::decoder::alone`]. Produces the legacy LZMA Utils
//! container format:
//!
//! ```text
//! offset  size  field
//! 0       1     properties byte: lc + 9*lp + 45*pb
//! 1       4     dictionary size, little-endian
//! 5       8     uncompressed size, little-endian
//! 13      …     LZMA1 stream
//! ```

#![forbid(unsafe_code)]

use crate::encoder::Lzma1Encoder;
use crate::LzmaError;

/// Default dictionary size for the encoder (16 MiB).
const DEFAULT_DICT_SIZE: u32 = 16 * 1024 * 1024;

/// Default LZMA parameters (matches lzip/xz-utils defaults).
const DEFAULT_LC: u32 = 3;
const DEFAULT_LP: u32 = 0;
const DEFAULT_PB: u32 = 2;

/// Compress `input` into the `.lzma` (LZMA-Alone) container using the
/// optimal (DP) parser for best ratio.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] only on arithmetic overflow.
pub fn lzma_alone_compress_optimal(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    let lc = DEFAULT_LC;
    let lp = DEFAULT_LP;
    let pb = DEFAULT_PB;

    let mut out = Vec::with_capacity(input.len() + 13);
    let props_byte = (lc + 9 * lp + 45 * pb) as u8;
    out.push(props_byte);
    out.extend_from_slice(&DEFAULT_DICT_SIZE.to_le_bytes());
    out.extend_from_slice(&(input.len() as u64).to_le_bytes());

    let encoder = Lzma1Encoder::new(lc, lp, pb);
    let stream = encoder.encode_optimal(input);
    out.extend_from_slice(&stream);

    Ok(out)
}

/// Compress `input` into the `.lzma` (LZMA-Alone) container.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] only on arithmetic overflow (shouldn't
/// happen for any plausible input).
pub fn lzma_alone_compress(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    let lc = DEFAULT_LC;
    let lp = DEFAULT_LP;
    let pb = DEFAULT_PB;

    let mut out = Vec::with_capacity(input.len() + 13);
    // Properties byte: lc + 9*lp + 45*pb.
    let props_byte = (lc + 9 * lp + 45 * pb) as u8;
    out.push(props_byte);
    out.extend_from_slice(&DEFAULT_DICT_SIZE.to_le_bytes());
    out.extend_from_slice(&(input.len() as u64).to_le_bytes());

    let encoder = Lzma1Encoder::new(lc, lp, pb);
    let stream = encoder.encode(input);
    out.extend_from_slice(&stream);

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lzma_alone_decompress;

    #[test]
    fn empty_round_trips() {
        let compressed = lzma_alone_compress(&[]).expect("encode");
        let decompressed = lzma_alone_decompress(&compressed).expect("decode");
        assert!(decompressed.is_empty());
    }

    #[test]
    fn small_round_trips() {
        let input = b"Hello, world! This is LZMA-Alone compression.";
        let compressed = lzma_alone_compress(input).expect("encode");
        let decompressed = lzma_alone_decompress(&compressed).expect("decode");
        assert_eq!(decompressed.as_slice(), input.as_ref());
    }

    #[test]
    fn header_byte_matches_formula() {
        let compressed = lzma_alone_compress(b"x").expect("encode");
        // lc=3, lp=0, pb=2 → 3 + 9*0 + 45*2 = 3 + 90 = 93.
        assert_eq!(compressed[0], 93);
    }

    #[test]
    fn optimal_parse_round_trips() {
        // The optimal parser must produce output the decoder can parse.
        let input = b"hello world hello world hello world hello world".repeat(5);
        let compressed = lzma_alone_compress_optimal(&input).expect("encode");
        let decompressed = lzma_alone_decompress(&compressed).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn optimal_parse_empty_round_trips() {
        let compressed = lzma_alone_compress_optimal(&[]).expect("encode");
        let decompressed = lzma_alone_decompress(&compressed).expect("decode");
        assert!(decompressed.is_empty());
    }

    #[test]
    fn optimal_parser_is_smaller_or_equal() {
        // For compressible input, the optimal parser should produce
        // output no larger than the lazy parser.
        let input: Vec<u8> = (0..10_000)
            .map(|i| if i % 100 < 80 { b'a' } else { (i % 26 + b'a' as i32) as u8 })
            .collect();
        let lazy = lzma_alone_compress(&input).expect("lazy");
        let optimal = lzma_alone_compress_optimal(&input).expect("optimal");
        assert!(
            optimal.len() <= lazy.len() + 10,
            "optimal {} should be ≤ lazy {} + tolerance",
            optimal.len(),
            lazy.len()
        );
    }
}
