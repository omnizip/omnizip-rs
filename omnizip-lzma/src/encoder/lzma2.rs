//! LZMA2 chunk encoder — wraps [`super::lzma1::Lzma1Encoder`] output
//! into the LZMA2 chunk format.
//!
//! Each chunk has a control byte + size fields + (optionally) LZMA
//! properties + LZMA1 stream. Chunks are concatenated and the stream
//! ends with a 0x00 control byte.

#![forbid(unsafe_code)]

use crate::LzmaError;

/// Default LZMA parameters (matches lzip/xz-utils defaults).
const DEFAULT_LC: u32 = 3;
const DEFAULT_LP: u32 = 0;
const DEFAULT_PB: u32 = 2;
#[allow(dead_code)] const DEFAULT_DICT_SIZE: u32 = 16 * 1024 * 1024;

/// Maximum uncompressed size per LZMA2 chunk: `(1 << 21) - 1`.
const MAX_CHUNK_UNCOMPRESSED: usize = (1 << 21) - 1;

/// Compress `input` as an LZMA2 stream. Uses one LZMA1 encoder per
/// chunk with state reset at the start (level 3 = full reset for the
/// first chunk).
pub fn encode_lzma2_stream(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
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

        // Encode chunk via Lzma1Encoder (which includes EOPM internally
        // for known-size encoding we can skip EOPM, but for simplicity
        // we always emit it — the LZMA2 decoder knows the chunk size
        // and stops accordingly).
        let encoder = crate::encoder::Lzma1Encoder::new(DEFAULT_LC, DEFAULT_LP, DEFAULT_PB);
        let compressed = encoder.encode(chunk);

        // Per LZMA2 spec, the compressed-size field excludes the
        // 5-byte range-coder flush. `bytes_for_decode` gives us the
        // pre-flush position.
        let usable_compressed_size = compressed.len().saturating_sub(5);
        if usable_compressed_size > u16::MAX as usize {
            return Err(LzmaError::Corrupt {
                reason: format!(
                    "LZMA2 chunk compressed size {usable_compressed_size} exceeds u16"
                ),
            });
        }
        let u_size = chunk_size - 1; // spec encodes size-1 in 21 bits
        let c_size = usable_compressed_size - 1; // spec encodes size-1 in 16 bits

        // Control byte: 0x80 | (reset_level << 5) | (u_size >> 16).
        // First chunk uses reset_level=3 (state + props + dict).
        // Subsequent chunks use reset_level=0.
        let reset_level: u8 = if first_chunk { 3 } else { 0 };
        let control: u8 = 0x80
            | (reset_level << 5)
            | ((u_size >> 16) as u8 & 0x1F);
        out.push(control);
        out.extend_from_slice(&((u_size & 0xFFFF) as u16).to_be_bytes());
        out.extend_from_slice(&((c_size & 0xFFFF) as u16).to_be_bytes());

        // For reset_level >= 2, emit properties byte.
        if reset_level >= 2 {
            let props = (DEFAULT_LC + 9 * DEFAULT_LP + 45 * DEFAULT_PB) as u8;
            out.push(props);
        }

        // Emit only the bytes_for_decode portion (exclude flush padding).
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
