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
//! cheapest representation among CONSTANT, VERBATIM, FIXED (orders
//! 0-4), and LPC (orders 1-32 with quantised coefficients).

#![forbid(unsafe_code)]

pub mod bitwriter;
pub mod fft;
pub mod frame;
pub mod lpc;
pub mod rice;
pub mod simd;
pub mod streaminfo;
pub mod subframe;

use crate::pcm_header::{Endianness, PcmParams};
use crate::streaminfo::StreamInfo;

/// FLAC stream magic bytes ("fLaC").
const FLAC_MAGIC: [u8; 4] = *b"fLaC";

/// Encode interleaved PCM audio as a full FLAC stream.
///
/// `input` is the raw PCM data, interleaved by channel. The format
/// (sample rate, channels, bps, endianness) is described by `params`.
///
/// Picks the block size via [`pick_block_size`], a heuristic that
/// balances frame overhead against LPC analysis quality. Use
/// [`encode_stream_best`] for the libFLAC `--best` behaviour (try
/// every candidate, return the smallest).
///
/// # Errors
///
/// Returns `String` on configuration errors or sample-reading failures.
pub fn encode_stream(input: &[u8], params: &PcmParams) -> Result<Vec<u8>, String> {
    let total_frames = validate_input(input, params)?;
    let block_size = pick_block_size(total_frames, params.sample_rate);
    encode_stream_with_block_size(input, params, block_size)
}

/// libFLAC `--best` semantics: try every candidate block size, return
/// the smallest output. Use [`encode_stream`] for the fast default.
///
/// # Errors
///
/// Returns `String` on configuration errors or sample-reading failures.
pub fn encode_stream_best(input: &[u8], params: &PcmParams) -> Result<Vec<u8>, String> {
    let total_frames = validate_input(input, params)?;

    // To avoid duplicate work, only candidates that produce DIFFERENT
    // clamped block sizes are tried.
    let mut seen_clamped: usize = 0;
    let mut best: Option<Vec<u8>> = None;
    for &block_size in CANDIDATE_BLOCK_SIZES {
        let clamped = block_size.min(total_frames.max(1));
        if clamped == seen_clamped {
            continue;
        }
        seen_clamped = clamped;
        match encode_stream_with_block_size(input, params, block_size) {
            Ok(encoded) => match &best {
                None => best = Some(encoded),
                Some(prev) if encoded.len() < prev.len() => best = Some(encoded),
                _ => {}
            },
            Err(_) => continue,
        }
    }
    best.ok_or_else(|| "no block size produced valid output".into())
}

/// Heuristic block-size picker. Mirrors libFLAC's stream-level default
/// for typical audio, with special-casing for very short or very long
/// inputs to avoid pathological per-block overhead.
///
/// | Condition | Block size | Rationale |
/// |-----------|-----------|----------|
/// | `total < 192` | `total.max(16)` | Tiny input — match |
/// | `total < 4096` | `256` | Small — keep frame header overhead low |
/// | `total < 65_536` | `4608` | libFLAC's mid-range default |
/// | `total ≥ 65_536`, `sr ≥ 44_100` | `4608` | Standard audio |
/// | `total ≥ 65_536`, `sr < 44_100` | `4096` | Power-of-two wins on low-rate |
#[must_use]
pub fn pick_block_size(total_frames: usize, sample_rate: u32) -> usize {
    if total_frames < 192 {
        return total_frames.max(16);
    }
    if total_frames < 4096 {
        return 256;
    }
    if total_frames < 65_536 {
        return 4608;
    }
    if sample_rate >= 44_100 {
        4608
    } else {
        4096
    }
}

/// Candidate block sizes tried by [`encode_stream_best`], ordered to
/// match libFLAC's `--best` sweep (small → large).
///
/// 4608 is libFLAC's mid-range default for 44.1 kHz; 4096/8192/16384
/// are the standard power-of-two options; 192/256/512 help very short
/// inputs where fixed overhead dominates.
const CANDIDATE_BLOCK_SIZES: &[usize] = &[192, 256, 512, 1024, 2048, 4096, 4608, 8192, 16384];

/// Encode interleaved PCM audio with a specific block size.
///
/// Use [`encode_stream`] for automatic block-size selection. This
/// function is exposed for callers that want to force a specific
/// block size (e.g. for testing or streaming).
///
/// # Errors
///
/// Returns `String` on configuration errors or sample-reading failures.
pub fn encode_stream_with_block_size(
    input: &[u8],
    params: &PcmParams,
    requested_block_size: usize,
) -> Result<Vec<u8>, String> {
    let mut channels_data = vec![Vec::new(); params.channels as usize];
    encode_stream_inner(input, params, requested_block_size, &mut channels_data)
}

/// Reusable-buffer variant: caller provides scratch `channels_data`
/// (one Vec per channel). Identical output to
/// [`encode_stream_with_block_size`]; saves the per-channel Vec
/// allocations on batch workloads.
///
/// # Errors
///
/// Returns `String` on configuration errors or sample-reading failures.
pub fn encode_stream_reusable(
    input: &[u8],
    params: &PcmParams,
    channels_data: &mut [Vec<i32>],
) -> Result<Vec<u8>, String> {
    let total_frames = validate_input(input, params)?;
    let block_size = pick_block_size(total_frames, params.sample_rate);
    encode_stream_inner(input, params, block_size, channels_data)
}

fn encode_stream_inner(
    input: &[u8],
    params: &PcmParams,
    requested_block_size: usize,
    channels_data: &mut [Vec<i32>],
) -> Result<Vec<u8>, String> {
    let bps = params.bits_per_sample;
    if !(4..=32).contains(&bps) {
        return Err(format!("unsupported bits_per_sample: {bps}"));
    }
    let channels = params.channels;
    if channels == 0 || channels > 8 {
        return Err(format!("unsupported channels: {channels}"));
    }
    let bytes_per_sample = usize::from(bps) / 8 + usize::from(usize::from(bps) % 8 > 0);
    let frame_bytes = usize::from(channels) * bytes_per_sample;
    if input.len() % frame_bytes != 0 {
        return Err(format!(
            "input len {} not a multiple of frame size {}",
            input.len(),
            frame_bytes
        ));
    }
    let total_frames = input.len() / frame_bytes;

    // De-interleave PCM into per-channel i32 samples. Reuse the
    // caller-provided buffers: truncate to len 0 (capacity preserved),
    // then push samples.
    deinterleave_into(input, params, bytes_per_sample, total_frames, channels_data)?;

    // Clamp block size to spec max (65535) and to total_frames.
    let block_size = requested_block_size.min(65535).min(total_frames.max(1));

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
        for chan in channels_data.iter() {
            block_channels.push(chan[offset..offset + this_block].to_vec());
        }

        frame::encode_frame(&mut writer, &block_channels, &info, frame_number)?;
        offset += this_block;
        frame_number += 1;
    }

    out.extend_from_slice(writer.as_bytes());
    Ok(out)
}

/// Validate input dimensions and return the total frame count.
fn validate_input(input: &[u8], params: &PcmParams) -> Result<usize, String> {
    let bps = params.bits_per_sample;
    if !(4..=32).contains(&bps) {
        return Err(format!("unsupported bits_per_sample: {bps}"));
    }
    let channels = params.channels;
    if channels == 0 || channels > 8 {
        return Err(format!("unsupported channels: {channels}"));
    }
    let bytes_per_sample = usize::from(bps) / 8 + usize::from(usize::from(bps) % 8 > 0);
    let frame_bytes = usize::from(channels) * bytes_per_sample;
    if input.len() % frame_bytes != 0 {
        return Err(format!(
            "input len {} not a multiple of frame size {}",
            input.len(),
            frame_bytes
        ));
    }
    Ok(input.len() / frame_bytes)
}

/// De-interleave raw PCM bytes into per-channel `i32` sample vectors.
///
/// Clears each channel Vec (preserving capacity) and refills it with
/// the new samples. The single-shot API that returns a fresh `Vec<Vec<i32>>`
/// was removed — all call sites use this reusable-buffer form to avoid
/// per-frame allocation.
fn deinterleave_into(
    input: &[u8],
    params: &PcmParams,
    bytes_per_sample: usize,
    total_frames: usize,
    channels_data: &mut [Vec<i32>],
) -> Result<(), String> {
    if channels_data.len() != params.channels as usize {
        return Err(format!(
            "channels_data len {} != params.channels {}",
            channels_data.len(),
            params.channels
        ));
    }
    for chan in channels_data.iter_mut() {
        chan.clear();
        chan.reserve(total_frames);
    }
    let frame_bytes = usize::from(params.channels) * bytes_per_sample;
    let is_le = matches!(params.endianness, Endianness::LittleEndian);

    for f in 0..total_frames {
        let frame_start = f * frame_bytes;
        for ch in 0..params.channels as usize {
            let sample_start = frame_start + ch * bytes_per_sample;
            let sample_bytes = &input[sample_start..sample_start + bytes_per_sample];
            let val = read_sample(
                sample_bytes,
                bytes_per_sample,
                is_le,
                params.bits_per_sample,
            );
            channels_data[ch].push(val);
        }
    }
    Ok(())
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
        assert!(
            encoded.len() < 100,
            "DC signal encoded to {} bytes",
            encoded.len()
        );
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

    #[test]
    fn pick_block_size_handles_tiny_input() {
        assert_eq!(pick_block_size(0, 8_000), 16);
        assert_eq!(pick_block_size(50, 8_000), 50);
        assert_eq!(pick_block_size(192, 8_000), 256);
    }

    #[test]
    fn pick_block_size_small_input_uses_256() {
        assert_eq!(pick_block_size(500, 8_000), 256);
        assert_eq!(pick_block_size(4_000, 44_100), 256);
    }

    #[test]
    fn pick_block_size_medium_input_uses_4608() {
        assert_eq!(pick_block_size(5_000, 44_100), 4608);
        assert_eq!(pick_block_size(60_000, 44_100), 4608);
    }

    #[test]
    fn pick_block_size_large_input_depends_on_sample_rate() {
        assert_eq!(pick_block_size(100_000, 44_100), 4608);
        assert_eq!(pick_block_size(100_000, 8_000), 4096);
    }

    #[test]
    fn encode_stream_best_round_trips() {
        let pcm = mono_sine(440.0, 8_000, 192);
        let params = PcmParams {
            sample_rate: 8_000,
            channels: 1,
            bits_per_sample: 16,
            endianness: Endianness::LittleEndian,
            sample_count: 192,
        };
        let encoded = encode_stream_best(&pcm, &params).expect("encode");
        let decoded = decoder::decode_stream(&encoded).expect("decode");
        assert_eq!(decoded, pcm);
    }

    #[test]
    fn encode_stream_uses_heuristic_block_size() {
        // The default encode_stream should pick a single block size
        // via the heuristic — measurable by speed relative to the
        // `--best` sweep.
        let pcm = mono_sine(440.0, 8_000, 8_192);
        let params = PcmParams {
            sample_rate: 8_000,
            channels: 1,
            bits_per_sample: 16,
            endianness: Endianness::LittleEndian,
            sample_count: 8_192,
        };

        let t_heur = std::time::Instant::now();
        let encoded_heur = encode_stream(&pcm, &params).expect("encode");
        let dt_heur = t_heur.elapsed();

        let t_best = std::time::Instant::now();
        let encoded_best = encode_stream_best(&pcm, &params).expect("encode");
        let dt_best = t_best.elapsed();

        // Heuristic should be at least 2× faster than the full sweep.
        assert!(
            dt_heur < dt_best / 2,
            "heuristic {:?} should be ≤ half of best {:?}",
            dt_heur,
            dt_best
        );

        // And ratio should be reasonable: heuristic output should be
        // within 50% of best.
        let ratio = encoded_heur.len() as f64 / encoded_best.len() as f64;
        assert!(
            ratio < 1.5,
            "heuristic output {} vs best {}",
            encoded_heur.len(),
            encoded_best.len()
        );

        // Both round-trip.
        let dec_heur = decoder::decode_stream(&encoded_heur).expect("decode");
        assert_eq!(dec_heur, pcm);
    }
}
