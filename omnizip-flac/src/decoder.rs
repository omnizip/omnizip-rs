//! Top-level FLAC stream decoder.
//!
//! Parses the `fLaC` stream format: magic + STREAMINFO metadata
//! block + optional metadata blocks + audio frames.

#![forbid(unsafe_code)]

use crate::bitreader::BitReader;
use crate::frame;
use crate::streaminfo::StreamInfo;

/// Decode a complete FLAC stream (starting with `fLaC` magic) into
/// interleaved PCM samples.
///
/// # Errors
///
/// Returns `String` on malformed data.
pub fn decode_stream(input: &[u8]) -> Result<Vec<u8>, String> {
    if input.len() < 4 {
        return Err("input too short for FLAC magic".into());
    }
    if &input[..4] != crate::FLAC_MAGIC {
        return Err("missing fLaC magic".into());
    }

    let mut pos = 4;

    // Parse metadata blocks. The first must be STREAMINFO.
    let mut info: Option<StreamInfo> = None;
    let mut is_last = false;

    while !is_last {
        if pos + 4 > input.len() {
            return Err("truncated metadata block header".into());
        }

        let header =
            u32::from_be_bytes([input[pos], input[pos + 1], input[pos + 2], input[pos + 3]]);
        is_last = (header >> 31) & 1 != 0;
        let block_type = (header >> 24) & 0x7F;
        let block_length = header & 0x00FF_FFFF;
        pos += 4;

        if pos + block_length as usize > input.len() {
            return Err("truncated metadata block".into());
        }

        match block_type {
            0 => {
                // STREAMINFO.
                info = StreamInfo::parse(&input[pos..pos + block_length as usize]);
                if info.is_none() {
                    return Err("invalid STREAMINFO block".into());
                }
            }
            _ => {
                // Skip other metadata blocks (VORBIS_COMMENT, etc.).
            }
        }

        pos += block_length as usize;
    }

    let info = info.ok_or("missing STREAMINFO metadata block")?;

    // Decode audio frames.
    let bps = info.bps();
    let channels = info.channel_count();
    let bytes_per_sample = (bps as usize + 7) / 8;

    let mut output: Vec<u8> = Vec::new();
    let mut total_samples_decoded: u64 = 0;

    while pos < input.len()
        && (info.total_samples == 0 || total_samples_decoded < info.total_samples)
    {
        let mut reader = BitReader::new(&input[pos..]);

        match frame::decode_frame(&mut reader, &info) {
            Ok((audio_frame, consumed)) => {
                // Interleave channel data into output.
                for sample_idx in 0..audio_frame.block_size {
                    for ch in 0..channels as usize {
                        if ch < audio_frame.channels.len()
                            && sample_idx < audio_frame.channels[ch].len()
                        {
                            let sample = audio_frame.channels[ch][sample_idx];
                            write_sample(&mut output, sample, bytes_per_sample);
                        }
                    }
                }
                total_samples_decoded += audio_frame.block_size as u64;
                pos += consumed;
            }
            Err(e) => {
                // If we've decoded enough samples, the remaining bytes
                // might be padding or padding artifacts.
                if total_samples_decoded > 0 {
                    break;
                }
                return Err(format!("frame decode error at offset {pos}: {e}"));
            }
        }
    }

    Ok(output)
}

/// Write a sample value as `bytes_per_sample` bytes, little-endian.
fn write_sample(out: &mut Vec<u8>, value: i32, bytes_per_sample: usize) {
    let unsigned = value as u32;
    match bytes_per_sample {
        1 => out.push(unsigned as u8),
        2 => out.extend_from_slice(&(unsigned as u16).to_le_bytes()),
        3 => {
            out.push(unsigned as u8);
            out.push((unsigned >> 8) as u8);
            out.push((unsigned >> 16) as u8);
        }
        4 => out.extend_from_slice(&unsigned.to_le_bytes()),
        _ => out.extend_from_slice(&unsigned.to_le_bytes()[..bytes_per_sample]),
    }
}

/// Check whether `input` starts with the FLAC magic bytes.
#[must_use]
pub fn is_flac_stream(input: &[u8]) -> bool {
    input.len() >= 4 && &input[..4] == b"fLaC"
}
