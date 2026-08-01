//! LZMA2 chunk decoder — drives [`crate::decoder::Lzma1Decoder`] with
//! state persistence across chunks.
//!
//! LZMA2 container format (XZ Utils `lzma2_decoder.c`):
//!
//! ```text
//! control  meaning
//! -------  -----------------------------------------------------------
//! 0x00     end of LZMA2 stream
//! 0x01     uncompressed chunk, reset dictionary
//! 0x02     uncompressed chunk, no reset
//! 0x80-FF  LZMA-compressed chunk:
//!            bits 5-6: reset state level
//!              0 = no reset (state persists from previous chunk)
//!              1 = reset state (models + rep distances)
//!              2 = reset state + read properties (lc/lp/pb)
//!              3 = reset state + read properties + reset dictionary
//!            bits 0-4: high 5 bits of uncompressed size
//! ```
//!
//! ## State persistence
//!
//! The LZMA1 decoder's probability models and rep distances persist
//! across "no reset" chunks (bits 5-6 = 0). The `Lzma1Decoder` is
//! owned by the LZMA2 decoder and reused across chunks; only the
//! range coder is recreated per chunk (every LZMA2 chunk has its own
//! range-coder initialisation).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::decoder::Lzma1Decoder;
use crate::LzmaError;

/// Decode a complete LZMA2 stream. Returns the decoded bytes and the
/// number of input bytes consumed.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] on truncation, invalid control
/// bytes, or any underlying LZMA1 decode error.
pub fn decode_lzma2_stream(input: &[u8]) -> Result<(Vec<u8>, usize), LzmaError> {
    let mut output = Vec::new();
    let mut cursor = 0usize;
    let mut decoder: Option<Lzma1Decoder> = None;
    let mut current_lc: u32 = 3;
    let mut current_lp: u32 = 0;
    let mut current_pb: u32 = 2;
    let dict_size: u32 = 1 << 24;

    loop {
        if cursor >= input.len() {
            return Err(LzmaError::Corrupt {
                reason: "LZMA2 stream truncated: missing control byte".into(),
            });
        }
        let control = input[cursor];
        cursor += 1;

        if control == 0 {
            break;
        }

        if control <= 2 {
            // Uncompressed chunk.
            if cursor + 2 > input.len() {
                return Err(LzmaError::Corrupt {
                    reason: "LZMA2 uncompressed chunk truncated".into(),
                });
            }
            let raw_size = u16::from_be_bytes([input[cursor], input[cursor + 1]]);
            cursor += 2;
            let size = usize::from(raw_size) + 1;
            if cursor + size > input.len() {
                return Err(LzmaError::Corrupt {
                    reason: format!(
                        "LZMA2 uncompressed chunk needs {size} bytes, got {}",
                        input.len() - cursor
                    ),
                });
            }
            output.extend_from_slice(&input[cursor..cursor + size]);
            cursor += size;
            continue;
        }

        if control < 0x80 {
            return Err(LzmaError::Corrupt {
                reason: format!("LZMA2 reserved control byte 0x{control:02X}"),
            });
        }

        // LZMA-compressed chunk (control >= 0x80).
        // Bits 5-6: reset level (0=none, 1=state, 2=state+props, 3=state+props+dict)
        let reset_level = (control >> 5) & 3;

        // Per XZ spec: the uncompressed size's high 5 bits come from
        // the control byte (bits 0-4); the low 16 bits come from the
        // next 2 bytes (big-endian). Total: 21 bits.
        if cursor + 4 > input.len() {
            return Err(LzmaError::Corrupt {
                reason: "LZMA2 compressed chunk header truncated".into(),
            });
        }
        let size_high = u32::from(control & 0x1F);
        let size_low = u32::from(u16::from_be_bytes([input[cursor], input[cursor + 1]]));
        let uncompressed_size = u64::from((size_high << 16) | size_low) + 1;
        let compressed_size =
            usize::from(u16::from_be_bytes([input[cursor + 2], input[cursor + 3]])) + 1;
        cursor += 4;

        // Read new properties if reset level >= 2.
        if reset_level >= 2 {
            if cursor >= input.len() {
                return Err(LzmaError::Corrupt {
                    reason: "LZMA2 properties byte truncated".into(),
                });
            }
            let props = u32::from(input[cursor]);
            cursor += 1;
            let pb_new = props / (9 * 5);
            let remainder = props - (pb_new * 9 * 5);
            let lp_new = remainder / 9;
            let lc_new = remainder - (lp_new * 9);
            if lc_new + lp_new > 4 {
                return Err(LzmaError::Corrupt {
                    reason: format!(
                        "LZMA2 properties lc({lc_new}) + lp({lp_new}) > 4"
                    ),
                });
            }
            current_lc = lc_new;
            current_lp = lp_new;
            current_pb = pb_new;
        }

        if cursor + compressed_size > input.len() {
            return Err(LzmaError::Corrupt {
                reason: format!(
                    "LZMA2 compressed chunk needs {compressed_size} bytes, got {}",
                    input.len() - cursor
                ),
            });
        }
        let chunk_data = &input[cursor..cursor + compressed_size];
        cursor += compressed_size;

        // Manage decoder state based on reset level.
        match reset_level {
            0 => {
                // No reset — reuse existing decoder. The first chunk
                // MUST have reset_level >= 1 per the spec; if we see
                // level 0 without a prior decoder, treat as corrupt.
                let d = decoder.as_mut().ok_or_else(|| LzmaError::Corrupt {
                    reason: "LZMA2 first chunk must reset state (level >= 1)".into(),
                })?;
                let start = output.len();
                d.decode_continuation(chunk_data, &mut output, uncompressed_size)?;
                let produced = output.len() - start;
                let _ = produced;
            }
            1 => {
                // Reset state (models + rep distances), keep lc/lp/pb.
                let d = decoder.get_or_insert_with(|| {
                    Lzma1Decoder::new(current_lc, current_lp, current_pb, dict_size)
                });
                d.reset_state();
                d.decode_continuation(chunk_data, &mut output, uncompressed_size)?;
            }
            2 | 3 => {
                // Reset state + new properties. Recreate the decoder.
                if reset_level == 3 {
                    // Full dictionary reset — clear output.
                    output.clear();
                }
                let mut d =
                    Lzma1Decoder::new(current_lc, current_lp, current_pb, dict_size);
                d.reset_state();
                d.decode_continuation(chunk_data, &mut output, uncompressed_size)?;
                decoder = Some(d);
            }
            _ => unreachable!("reset_level is masked to 2 bits"),
        }
    }

    Ok((output, cursor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stream_returns_empty() {
        let (out, consumed) = decode_lzma2_stream(&[0x00]).expect("decode");
        assert!(out.is_empty());
        assert_eq!(consumed, 1);
    }

    #[test]
    fn truncated_stream_errors() {
        assert!(decode_lzma2_stream(&[]).is_err());
    }

    #[test]
    fn unknown_control_byte_errors() {
        assert!(decode_lzma2_stream(&[0x03]).is_err());
        assert!(decode_lzma2_stream(&[0x40]).is_err());
        assert!(decode_lzma2_stream(&[0x7F]).is_err());
    }

    #[test]
    fn uncompressed_chunk_decodes() {
        let mut stream = vec![0x01, 0x00, 0x04];
        stream.extend_from_slice(b"Hello");
        stream.push(0x00);
        let (out, consumed) = decode_lzma2_stream(&stream).expect("decode");
        assert_eq!(out, b"Hello");
        assert_eq!(consumed, stream.len());
    }
}
