//! FLAC stream encoder — produces real `fLaC` bitstreams.
//!
//! ## Layout
//!
//! ```text
//! "fLaC" magic (4 bytes)
//! STREAMINFO metadata block (38 bytes: 4-byte header + 34-byte body)
//! [optional: more metadata blocks — PADDING, SEEKTABLE, etc.]
//! Audio frames (one per block of samples)
//! ```
//!
//! ## Pipeline
//!
//! 1. De-interleave input PCM into per-channel sample vectors.
//! 2. Split into fixed-size blocks (default: 4096 samples per block).
//! 3. Encode each block as a frame (header + subframes + footer).
//!
//! For each channel within a frame, the subframe encoder picks the
//! cheapest representation among CONSTANT, VERBATIM, and FIXED
//! (orders 0-4). LPC subframe type is planned (see TODO 62).

#![forbid(unsafe_code)]

pub mod bitwriter;
pub mod frame;
pub mod rice;
pub mod streaminfo;
pub mod subframe;

use crate::pcm_header::{Endianness, PcmParams};
use crate::streaminfo::StreamInfo;

/// FLAC stream magic bytes ("fLaC").
const FLAC_MAGIC: [u8; 4] = *b"fLaC";

/// Default block size in samples per channel.
const DEFAULT_BLOCK_SIZE: usize = 4096;

/// Encode interleaved PCM audio as a full FLAC stream.
///
/// `input` is the raw PCM data, interleaved by channel. The format
/// (sample rate, channels, bps, endianness) is described by `params`.
///
/// # Errors
///
/// Returns `String` on configuration errors or sample-reading failures.
pub fn encode_stream(input: &[u8], params: &PcmParams) -> Result<Vec<u8>, String> {
    let bps = params.bits_per_sample;
    if bps < 4 || bps > 32 {
        return Err(format!("unsupported bits_per_sample: {bps}"));
    }
    let channels = params.channels;
    if channels == 0 || channels > 8 {
        return Err(format!("unsupported channels: {channels}"));
    }
    let bytes_per_sample = usize::from(bps) / 8 + if usize::from(bps) % 8 > 0 { 1 } else { 0 };
    let frame_bytes = usize::from(channels) * bytes_per_sample;
    if input.len() % frame_bytes != 0 {
        return Err(format!(
            "input len {} not a multiple of frame size {}",
            input.len(),
            frame_bytes
        ));
    }
    let total_frames = input.len() / frame_bytes;

    // De-interleave PCM into per-channel i32 samples.
    let channels_data = deinterleave(input, params, bytes_per_sample, total_frames)?;

    // Split into blocks and encode.
    let block_size = DEFAULT_BLOCK_SIZE.min(total_frames.max(1));
    let mut out = Vec::with_capacity(42 + input.len() / 2);
    out.extend_from_slice(&FLAC_MAGIC);

    // STREAMINFO metadata block.
    let streaminfo_block = streaminfo::build_streaminfo_block(
        block_size as u16,
        block_size as u16,
        params.sample_rate,
        channels,
        bps,
        total_frames as u64,
        [0u8; 16], // MD5 not computed — left zero (valid per spec).
    );
    out.extend_from_slice(&streaminfo_block);

    // Build a StreamInfo for the frame encoder to read.
    let info = StreamInfo {
        min_block_size: block_size as u32,
        max_block_size: block_size as u32,
        min_frame_size: 0,
        max_frame_size: 0,
        sample_rate: params.sample_rate,
        channels: channels - 1,
        bits_per_sample: bps - 1,
        total_samples: total_frames as u64,
        md5: [0u8; 16],
    };

    // Encode blocks.
    let mut writer = bitwriter::BitWriter::with_capacity(input.len() / 2);
    let mut offset = 0usize;
    let mut frame_number = 0u32;
    while offset < total_frames {
        let remaining = total_frames - offset;
        let this_block = block_size.min(remaining);

        let mut block_channels = Vec::with_capacity(channels as usize);
        for chan in &channels_data {
            block_channels.push(chan[offset..offset + this_block].to_vec());
        }

        frame::encode_frame(&mut writer, &block_channels, &info, frame_number)?;
        offset += this_block;
        frame_number += 1;
    }

    out.extend_from_slice(&writer.as_bytes());
    Ok(out)
}

/// De-interleave raw PCM bytes into per-channel i32 sample vectors.
///
/// Input layout (LE example, 16-bit stereo):
///   `[L_lo, L_hi, R_lo, R_hi, L_lo, L_hi, R_lo, R_hi, ...]`
///
/// Output: `vec![vec![L0, L1, ...], vec![R0, R1, ...]]`
fn deinterleave(
    input: &[u8],
    params: &PcmParams,
    bytes_per_sample: usize,
    total_frames: usize,
) -> Result<Vec<Vec<i32>>, String> {
    let mut channels_data = vec![Vec::with_capacity(total_frames); params.channels as usize];
    let frame_bytes = usize::from(params.channels) * bytes_per_sample;
    let is_le = matches!(params.endianness, Endianness::LittleEndian);

    for f in 0..total_frames {
        let frame_start = f * frame_bytes;
        for ch in 0..params.channels as usize {
            let sample_start = frame_start + ch * bytes_per_sample;
            let sample_bytes = &input[sample_start..sample_start + bytes_per_sample];
            let val = read_sample(sample_bytes, bytes_per_sample, is_le, params.bits_per_sample);
            channels_data[ch].push(val);
        }
    }

    Ok(channels_data)
}

/// Read one PCM sample from `bytes`, sign-extending as needed.
fn read_sample(bytes: &[u8], nbytes: usize, is_le: bool, bps: u8) -> i32 {
    // Build the unsigned value.
    let mut val: u32 = 0;
    if is_le {
        for i in 0..nbytes {
            val |= u32::from(bytes[i]) << (i * 8);
        }
    } else {
        for i in 0..nbytes {
            val |= u32::from(bytes[nbytes - 1 - i]) << (i * 8);
        }
    }

    // Sign-extend if the top bit is set.
    let top_bit = bps - 1;
    if bps < 32 && val & (1 << top_bit) != 0 {
        let mask = u32::MAX << bps;
        val |= mask;
    }
    val as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder;
    use crate::pcm_header::{Endianness, PcmParams};

    fn mono_sine(freq: f64, sr: u32, n: usize) -> Vec<u8> {
        let mut pcm = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = i as f64 / sr as f64;
            let s = (t * freq * std::f64::consts::TAU).sin() * 10_000.0;
            let v = (s as i16).to_le_bytes();
            pcm.push(v[0]);
            pcm.push(v[1]);
        }
        pcm
    }

    #[test]
    fn encode_then_decode_sine() {
        let pcm = mono_sine(440.0, 8_000, 192);
        let params = PcmParams {
            sample_rate: 8_000,
            channels: 1,
            bits_per_sample: 16,
            endianness: Endianness::LittleEndian,
            sample_count: 192,
        };
        let encoded = encode_stream(&pcm, &params).expect("encode");
        assert_eq!(&encoded[..4], &FLAC_MAGIC);

        let decoded = decoder::decode_stream(&encoded).expect("decode");
        assert_eq!(decoded.len(), pcm.len());

        // Verify PCM round-trips bit-exactly.
        for (i, (a, b)) in decoded.iter().zip(pcm.iter()).enumerate() {
            assert_eq!(*a, *b, "sample {i} mismatch: {} vs {}", *a, *b);
        }
    }

    #[test]
    fn encode_dc_signal_uses_constant() {
        // All-zero PCM → CONSTANT subframe → very compact output.
        let pcm = vec![0u8; 384]; // 192 frames × 2 bytes
        let params = PcmParams {
            sample_rate: 8_000,
            channels: 1,
            bits_per_sample: 16,
            endianness: Endianness::LittleEndian,
            sample_count: 192,
        };
        let encoded = encode_stream(&pcm, &params).expect("encode");
        // STREAMINFO (42) + frame (~10-20 bytes for CONSTANT) → well under 100.
        assert!(encoded.len() < 100, "DC signal encoded to {} bytes", encoded.len());
    }

    #[test]
    fn encode_stereo_round_trips() {
        let pcm = vec![0u8; 192 * 4]; // stereo 16-bit, 192 frames
        let params = PcmParams {
            sample_rate: 8_000,
            channels: 2,
            bits_per_sample: 16,
            endianness: Endianness::LittleEndian,
            sample_count: 192,
        };
        let encoded = encode_stream(&pcm, &params).expect("encode");
        let decoded = decoder::decode_stream(&encoded).expect("decode");
        assert_eq!(decoded, pcm);
    }

    #[test]
    fn determinism() {
        let pcm = mono_sine(440.0, 8_000, 192);
        let params = PcmParams {
            sample_rate: 8_000,
            channels: 1,
            bits_per_sample: 16,
            endianness: Endianness::LittleEndian,
            sample_count: 192,
        };
        let a = encode_stream(&pcm, &params).expect("encode");
        let b = encode_stream(&pcm, &params).expect("encode");
        assert_eq!(a, b);
    }

    #[test]
    fn read_sample_sign_extends() {
        // 16-bit LE sample 0xFF80 = -128.
        let bytes = [0x80, 0xFF];
        let val = read_sample(&bytes, 2, true, 16);
        assert_eq!(val, -128);

        // 16-bit LE sample 0x0080 = +128.
        let bytes = [0x80, 0x00];
        let val = read_sample(&bytes, 2, true, 16);
        assert_eq!(val, 128);
    }
}
