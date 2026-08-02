//! Lzip container encoder — wraps LZMA1 with the lzip format.
//!
//! Lzip uses fixed LZMA parameters (lc=3, lp=0, pb=2) and includes
//! a per-member trailer with CRC32 + `data_size` + `member_size`.

#![forbid(unsafe_code)]

use crate::crc32::crc32;
use crate::encoder::Lzma1Encoder;
use crate::lzip::{decode_dict_size, LZIP_HEADER_SIZE, LZIP_MAGIC, LZIP_TRAILER_SIZE};
use crate::LzmaError;

const LZIP_LC: u32 = 3;
const LZIP_LP: u32 = 0;
const LZIP_PB: u32 = 2;

/// Dict size code that decodes to 4 MiB (lzip's typical default).
const DICT_SIZE: u32 = 4 * 1024 * 1024;

/// Encode the `dict_size` byte that lzip's decoder expects. Inverse
/// of `lzip::decode_dict_size`.
fn encode_dict_size_byte(dict_size: u32) -> u8 {
    // Find the largest code such that decode_dict_size(code) >= dict_size.
    for code in (0..=40u8).rev() {
        if decode_dict_size(code) >= dict_size {
            return code;
        }
    }
    40
}

/// Compress `input` as a single-member lzip file.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] only on arithmetic overflow.
pub fn lzip_compress(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    let mut out = Vec::with_capacity(input.len() + 32);

    // Header: magic + version (1) + dict_size_code.
    out.extend_from_slice(&LZIP_MAGIC);
    out.push(1); // version 1
    let dict_size_byte = encode_dict_size_byte(DICT_SIZE);
    out.push(dict_size_byte);

    // LZMA1 stream (with EOPM).
    let encoder = Lzma1Encoder::new(LZIP_LC, LZIP_LP, LZIP_PB);
    let lzma_stream = encoder.encode(input);

    // Lzip trailer (20 bytes): CRC32 + data_size (LE u64) + member_size (LE u64).
    let crc = crc32(input);
    let data_size = input.len() as u64;
    let member_size = (LZIP_HEADER_SIZE + lzma_stream.len() + LZIP_TRAILER_SIZE) as u64;

    out.extend_from_slice(&lzma_stream);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&data_size.to_le_bytes());
    out.extend_from_slice(&member_size.to_le_bytes());

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lzip::lzip_decompress;

    #[test]
    fn empty_round_trips() {
        let compressed = lzip_compress(&[]).expect("encode");
        let decompressed = lzip_decompress(&compressed).expect("decode");
        assert!(decompressed.is_empty());
    }

    #[test]
    fn small_round_trips() {
        let input = b"hello lzip world";
        let compressed = lzip_compress(input).expect("encode");
        let decompressed = lzip_decompress(&compressed).expect("decode");
        assert_eq!(decompressed.as_slice(), input.as_ref());
    }

    #[test]
    fn header_is_correct() {
        let compressed = lzip_compress(b"x").expect("encode");
        assert_eq!(&compressed[..4], &LZIP_MAGIC);
        assert_eq!(compressed[4], 1); // version
    }

    #[test]
    fn determinism() {
        let encode_once = || lzip_compress(b"determinism").unwrap();
        assert_eq!(encode_once(), encode_once());
    }
}
