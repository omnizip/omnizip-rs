//! LZMA2 chunk encoder — wraps [`super::lzma1::Lzma1Encoder`] output
//! into the LZMA2 chunk format.
//!
//! Each chunk has a control byte + size fields + (optionally) LZMA
//! properties + LZMA1 stream. Chunks are concatenated and the stream
//! ends with a 0x00 control byte.

#![forbid(unsafe_code)]

use crate::LzmaError;
use super::alone::LzmaOptions;

/// Default LZMA parameters (matches lzip/xz-utils defaults).
#[allow(dead_code)]
const DEFAULT_LC: u32 = 3;
#[allow(dead_code)]
const DEFAULT_LP: u32 = 0;
#[allow(dead_code)]
const DEFAULT_PB: u32 = 2;
#[allow(dead_code)] const DEFAULT_DICT_SIZE: u32 = 16 * 1024 * 1024;

/// Maximum uncompressed size per LZMA2 chunk: `(1 << 21) - 1`.
const MAX_CHUNK_UNCOMPRESSED: usize = (1 << 21) - 1;

/// Compress `input` as an LZMA2 stream. Uses one LZMA1 encoder per
/// chunk with state reset at the start (level 3 = full reset for the
/// first chunk).
pub fn encode_lzma2_stream(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    encode_lzma2_stream_with_options(input, &LzmaOptions::default())
}

/// Compress `input` as an LZMA2 stream with explicit LZMA parameters
/// (lc, lp, pb). The `dict_size` field of `options` is used for
/// validation only — LZMA2 chunks always use the encoder's internal
/// dict size, which is fixed by `MAX_CHUNK_UNCOMPRESSED`.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] on parameter validation failure.
pub fn encode_lzma2_stream_with_options(
    input: &[u8],
    options: &LzmaOptions,
) -> Result<Vec<u8>, LzmaError> {
    options.validate()?;
    let lc = options.lc;
    let lp = options.lp;
    let pb = options.pb;
    let use_optimal = options.use_optimal_parser;
    let mut out = Vec::new();

    if input.is_empty() {
        // Empty input: single end-of-stream byte.
        out.push(0x00);
        return Ok(out);
    }

    let mut offset = 0;
    let mut first_chunk = true;
    while offset < input.len() {
        let remaining = input.len() - offset;
        let chunk_size = remaining.min(MAX_CHUNK_UNCOMPRESSED);
        let chunk = &input[offset..offset + chunk_size];

        let encoder = crate::encoder::Lzma1Encoder::new(lc, lp, pb);
        let compressed = if use_optimal {
            encoder.encode_optimal_with_tuning(
                chunk,
                options.max_chain_length,
                options.nice_match,
            )
        } else {
            encoder.encode_with_tuning(chunk, options.max_chain_length, options.nice_match)
        };

        let usable_compressed_size = compressed.len().saturating_sub(5);
        if usable_compressed_size > u16::MAX as usize {
            return Err(LzmaError::Corrupt {
                reason: format!(
                    "LZMA2 chunk compressed size {usable_compressed_size} exceeds u16"
                ),
            });
        }
        let u_size = chunk_size - 1;
        let c_size = usable_compressed_size - 1;

        let reset_level: u8 = if first_chunk { 3 } else { 0 };
        let control: u8 = 0x80
            | (reset_level << 5)
            | ((u_size >> 16) as u8 & 0x1F);
        out.push(control);
        out.extend_from_slice(&((u_size & 0xFFFF) as u16).to_be_bytes());
        out.extend_from_slice(&((c_size & 0xFFFF) as u16).to_be_bytes());

        if reset_level >= 2 {
            let props = (lc + 9 * lp + 45 * pb) as u8;
            out.push(props);
        }

        out.extend_from_slice(&compressed[..usable_compressed_size]);

        offset += chunk_size;
        first_chunk = false;
    }

    // End-of-stream marker.
    out.push(0x00);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lzma2::decode_lzma2_stream;

    #[test]
    fn empty_input_round_trips() {
        let compressed = encode_lzma2_stream(&[]).expect("encode");
        let (out, consumed) = decode_lzma2_stream(&compressed).expect("decode");
        assert!(out.is_empty());
        assert_eq!(consumed, 1); // just the EOS byte
    }

    #[test]
    fn small_input_round_trips() {
        let input = b"hello LZMA2 world";
        let compressed = encode_lzma2_stream(input).expect("encode");
        let (out, _) = decode_lzma2_stream(&compressed).expect("decode");
        assert_eq!(out, input);
    }

    #[test]
    fn determinism() {
        let encode_once = || encode_lzma2_stream(b"determinism test").unwrap();
        assert_eq!(encode_once(), encode_once());
    }
}
