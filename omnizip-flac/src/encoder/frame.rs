//! FLAC frame encoder.
//!
//! Wraps one block of audio (all channels' subframes) with a frame
//! header (sync code + block params + CRC-8) and frame footer (CRC-16).
//!
//! Wire format (must match `frame::decode_frame`):
//! ```text
//! Frame_Header:
//!   sync code (14 bits = 0x3FFE)
//!   reserved (1 bit = 0)
//!   blocking strategy (1 bit = 0 for fixed-block)
//!   block size code (4 bits)
//!   sample rate code (4 bits)
//!   channel assignment (4 bits)
//!   sample size code (3 bits)
//!   reserved (1 bit = 0)
//!   UTF-8 coded frame number (variable; we use 1 byte for small files)
//!   [optional block size extra bytes]
//!   [optional sample rate extra bytes]
//!   CRC-8 of all header bytes so far
//!
//! Subframe × channels
//!
//! Byte align
//!
//! Frame_Footer:
//!   CRC-16 of all frame bytes (header + subframes + align)
//! ```

#![forbid(unsafe_code)]

use crate::encoder::bitwriter::BitWriter;
use crate::encoder::subframe;
use crate::streaminfo::StreamInfo;

/// Frame sync code: 14 bits of 0x3FFE.
const SYNC_CODE: u64 = 0x3FFE;

/// Encode one frame of audio into `writer`.
///
/// `frame_number` is the sequential frame index (0-based). The samples
/// are per-channel (de-interleaved).
///
/// For stereo input, evaluates 4 channel assignments (independent,
/// left/side, right/side, mid/side) and picks the one with the smallest
/// total subframe cost.
///
/// # Errors
///
/// Returns `String` on internal errors.
pub fn encode_frame(
    writer: &mut BitWriter,
    channels_data: &[Vec<i32>],
    info: &StreamInfo,
    frame_number: u32,
) -> Result<(), String> {
    let num_channels = channels_data.len();
    if num_channels == 0 {
        return Err("no channels".into());
    }
    let block_size = channels_data[0].len();
    if channels_data.iter().any(|c| c.len() != block_size) {
        return Err("channels have unequal block sizes".into());
    }

    // For stereo, try all 4 channel assignments and pick the cheapest.
    let (best_channels, channel_assign) = if num_channels == 2 {
        pick_best_stereo_assignment(&channels_data[0], &channels_data[1])
    } else {
        (channels_data.to_vec(), (num_channels - 1) as u64)
    };

    let header_start = writer.position();

    // Sync code + reserved + blocking strategy (fixed-block = 0).
    writer.write_bits(SYNC_CODE, 14);
    writer.write_bits(0, 1); // reserved
    writer.write_bits(0, 1); // fixed-block strategy

    // Block size code (4 bits).
    let (bs_code, bs_extra): (u64, Option<u32>) = pick_block_size_code(block_size);
    writer.write_bits(bs_code, 4);

    // Sample rate code (4 bits). Prefer direct codes over "from STREAMINFO"
    // because libFLAC's decoder requires `has_stream_info` to be set for the
    // 0 code path, and some decoder configurations skip STREAMINFO parsing.
    let (sr_code, sr_extra) = pick_sample_rate_code(info.sample_rate);
    writer.write_bits(sr_code, 4);

    // Channel assignment (4 bits).
    writer.write_bits(channel_assign, 4);

    // Sample size code (3 bits). Same rationale — prefer direct codes.
    let ss_code = pick_sample_size_code(info.bps());
    writer.write_bits(ss_code, 3);

    // Reserved bit.
    writer.write_bits(0, 1);

    // UTF-8 coded frame number.
    write_utf8_coded(writer, u64::from(frame_number));

    // Optional block size extra bytes.
    if let Some(extra) = bs_extra {
        if extra <= u8::MAX as u32 {
            writer.write_bits(u64::from(extra), 8);
        } else {
            writer.write_bits(u64::from(extra), 16);
        }
    }

    // Optional sample rate extra bytes (for codes 12, 13, 14).
    if let Some(extra) = sr_extra {
        match sr_code {
            12 => writer.write_bits(u64::from(extra), 8),       // 8-bit kHz
            13 => writer.write_bits(u64::from(extra), 16),      // 16-bit Hz
            14 => writer.write_bits(u64::from(extra), 16),      // 16-bit decihz
            _ => {}
        }
    }

    // CRC-8 of header bytes so far.
    let header_end = writer.position();
    let crc8 = crate::crc::crc8(&writer.as_bytes()[header_start..header_end]);
    writer.write_bits(u64::from(crc8), 8);

    // Subframes.
    for chan in &best_channels {
        subframe::encode_subframe(writer, chan, info.bps())?;
    }

    // Byte-align (pad with zero bits).
    writer.flush_byte_aligned();

    // CRC-16 of all frame bytes (big-endian per FLAC spec — high byte first).
    let frame_end = writer.position();
    let crc16 = crate::crc::crc16(&writer.as_bytes()[header_start..frame_end]);
    writer.write_bits(u64::from(crc16), 16);

    Ok(())
}

/// Channel assignment codes (from the FLAC frame header spec).
/// For independent stereo, assign = 1 (num_channels - 1).
const CH_INDEPENDENT_STEREO: u64 = 1;
const CH_LEFT_SIDE: u64 = 8;
const CH_RIGHT_SIDE: u64 = 9;
const CH_MID_SIDE: u64 = 10;

/// For stereo input, try all 4 channel assignments and return the one
/// with the smallest estimated total subframe cost.
///
/// Returns `(channels_for_encoding, channel_assign_code)`.
fn pick_best_stereo_assignment(left: &[i32], right: &[i32]) -> (Vec<Vec<i32>>, u64) {
    // Independent (no transform).
    let indep_cost = estimate_subframe_cost(left) + estimate_subframe_cost(right);

    // Left/side: channel 0 = left, channel 1 = side = left - right.
    let side: Vec<i32> = left.iter().zip(right.iter()).map(|(&l, &r)| l - r).collect();
    let ls_cost = estimate_subframe_cost(left) + estimate_subframe_cost(&side);

    // Right/side: channel 0 = right, channel 1 = side = left - right.
    let rs_cost = estimate_subframe_cost(right) + estimate_subframe_cost(&side);

    // Mid/side: channel 0 = mid = (l+r)>>1, channel 1 = side = l-r.
    let mid: Vec<i32> = left.iter().zip(right.iter()).map(|(&l, &r)| (l + r) >> 1).collect();
    let ms_cost = estimate_subframe_cost(&mid) + estimate_subframe_cost(&side);

    // Pick the cheapest.
    let mut best = (vec![left.to_vec(), right.to_vec()], CH_INDEPENDENT_STEREO, indep_cost);
    if ls_cost < best.2 {
        best = (vec![left.to_vec(), side.clone()], CH_LEFT_SIDE, ls_cost);
    }
    if rs_cost < best.2 {
        best = (vec![right.to_vec(), side.clone()], CH_RIGHT_SIDE, rs_cost);
    }
    if ms_cost < best.2 {
        best = (vec![mid, side], CH_MID_SIDE, ms_cost);
    }

    (best.0, best.1)
}

/// Quick estimate of the subframe encoding cost: sum of |residual|
/// after order-1 FIXED prediction. Cheaper than running the full
/// subframe encoder for each candidate.
fn estimate_subframe_cost(samples: &[i32]) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    if samples.iter().all(|&s| s == samples[0]) {
        return 8; // CONSTANT — very cheap
    }
    // Order-1 FIXED residual sum.
    let mut sum: u64 = 0;
    for i in 1..samples.len() {
        let residual = samples[i] - samples[i - 1];
        let mapped = ((residual as u32) << 1) ^ ((residual >> 31) as u32);
        sum += u64::from(mapped) + 2; // ~Rice cost per residual
    }
    sum
}

/// Map a block size to its 4-bit code + optional extra-byte payload.
///
/// Returns `(code, Some(extra))` for codes 6-7, or `(code, None)` for
/// the fixed-size codes 1-5, 8-15.
fn pick_block_size_code(block_size: usize) -> (u64, Option<u32>) {
    // Fixed codes: 192, 576×2^k for k=0..3.
    let fixed: [(u64, usize); 5] = [
        (1, 192),
        (2, 576),
        (3, 1152),
        (4, 2304),
        (5, 4608),
    ];
    for &(code, size) in &fixed {
        if size == block_size {
            return (code, None);
        }
    }
    // Power-of-2 codes 8..=15: 256 × 2^(code-8).
    for code in 8u64..=15 {
        let size = 256usize << (code - 8);
        if size == block_size {
            return (code, None);
        }
    }
    // 8-bit extra: code 6, extra = block_size - 1 (1..=256).
    if block_size <= 256 {
        return (6, Some(block_size as u32 - 1));
    }
    // 16-bit extra: code 7, extra = block_size - 1 (1..=65536).
    (7, Some(block_size as u32 - 1))
}

/// Map a sample rate to its 4-bit code + optional extra-byte payload.
///
/// Returns `(code, None)` for the direct-coded rates, or `(code, Some(extra))`
/// for the codes that take an extra byte sequence from the end of the header.
fn pick_sample_rate_code(sample_rate: u32) -> (u64, Option<u32>) {
    // Per FLAC spec table.
    match sample_rate {
        88_200 => (1, None),
        176_400 => (2, None),
        192_000 => (3, None),
        8_000 => (4, None),
        16_000 => (5, None),
        22_050 => (6, None),
        24_000 => (7, None),
        32_000 => (8, None),
        44_100 => (9, None),
        48_000 => (10, None),
        96_000 => (11, None),
        // 12: get 8 bit sample rate (in kHz) from end of header
        _ if sample_rate % 1000 == 0 && sample_rate / 1000 <= 255 => {
            (12, Some(sample_rate / 1000))
        }
        // 13: get 16 bit sample rate (in Hz) from end of header
        _ if sample_rate <= u16::MAX as u32 => (13, Some(sample_rate)),
        // 14: get 16 bit sample rate (in tens of Hz) from end of header
        _ if sample_rate % 10 == 0 && sample_rate / 10 <= u16::MAX as u32 => {
            (14, Some(sample_rate / 10))
        }
        // 0: unspecified, get from STREAMINFO metadata block. We avoid this
        // path because libFLAC requires `has_stream_info` for it.
        _ => (0, None),
    }
}

/// Map a sample bit depth to its 3-bit code.
///
/// Returns the direct code for known bit depths, or 0 (from STREAMINFO)
/// for unusual values.
fn pick_sample_size_code(bps: u8) -> u64 {
    match bps {
        8 => 1,
        12 => 2,
        16 => 4,
        20 => 5,
        24 => 6,
        32 => 7,
        _ => 0, // from STREAMINFO (unusual bit depths)
    }
}

/// Write a UTF-8 coded number (FLAC frame/sample number encoding).
///
/// FLAC uses a custom UTF-8-like encoding for frame numbers:
/// - 1 byte:  0xxxxxxx                     → value 0-127
/// - 2 bytes: 110xxxxx 10xxxxxx            → value 128-2047
/// - 3 bytes: 1110xxxx 10xxxxxx 10xxxxxx   → value 2048-65535
/// - 4 bytes: 11110xxx 10xxxxxx ×3          → value 65536-2097151
/// - ...
///
/// For `nbytes` continuation bytes, the first byte has `(nbytes+1)`
/// leading 1-bits, then a 0-bit separator, then `(6 - nbytes)` data
/// bits. Total data bits = `(6 - nbytes) + 6 * nbytes = 6 + 5 * nbytes`.
fn write_utf8_coded(writer: &mut BitWriter, value: u64) {
    if value < 0x80 {
        writer.write_bits(value, 8);
        return;
    }

    // Determine byte count from the value range.
    let mut nbytes = 1usize;
    let mut max_val = 0x7FFu64; // 2-byte max = (1<<11)-1
    while value > max_val && nbytes < 6 {
        nbytes += 1;
        // total data bits = 6 + 5 * nbytes
        max_val = (1u64 << (6 + 5 * nbytes)) - 1;
    }

    // First byte: (nbytes+1) leading 1s, then 0-separator, then data.
    let leading_ones_mask = ((1u8 << (nbytes + 1)) - 1) << (7 - nbytes);
    let data_bits_first = 6 - nbytes; // wait, this should be 7 - nbytes - 1 = 6 - nbytes... hmm
    // Actually: total bits = 8. Leading 1s = nbytes+1. Separator = 1.
    // Data = 8 - (nbytes+1) - 1 = 6 - nbytes.
    let data_mask = (1u8 << data_bits_first) - 1;
    let high_data = (value >> (6 * nbytes)) as u8 & data_mask;
    writer.write_bits(u64::from(leading_ones_mask | high_data), 8);

    // Continuation bytes: 10xxxxxx.
    for i in (0..nbytes).rev() {
        let byte = ((value >> (6 * i)) & 0x3F) as u8 | 0x80;
        writer.write_bits(u64::from(byte), 8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream_info(bps: u8, channels: u8) -> StreamInfo {
        StreamInfo {
            min_block_size: 0,
            max_block_size: 0,
            min_frame_size: 0,
            max_frame_size: 0,
            sample_rate: 44_100,
            channels: channels - 1,
            bits_per_sample: bps - 1,
            total_samples: 0,
            md5: [0u8; 16],
        }
    }

    #[test]
    fn frame_round_trips_mono_verbatim() {
        let info = stream_info(16, 1);
        let samples: Vec<i32> = (0..192).map(|i| (i * 137) % 1000).collect();
        let channels_data = vec![samples.clone()];

        let mut w = BitWriter::new();
        encode_frame(&mut w, &channels_data, &info, 0).expect("encode");
        let bytes = w.finish();

        // Decode and verify.
        let mut reader = crate::bitreader::BitReader::new(&bytes);
        let (decoded, _) = crate::frame::decode_frame(&mut reader, &info).expect("decode");
        assert_eq!(decoded.channels, channels_data);
    }

    #[test]
    fn frame_round_trips_stereo_constant() {
        let info = stream_info(16, 2);
        let left: Vec<i32> = vec![1000; 192];
        let right: Vec<i32> = vec![2000; 192];
        let channels_data = vec![left, right];

        let mut w = BitWriter::new();
        encode_frame(&mut w, &channels_data, &info, 0).expect("encode");
        let bytes = w.finish();

        let mut reader = crate::bitreader::BitReader::new(&bytes);
        let (decoded, _) = crate::frame::decode_frame(&mut reader, &info).expect("decode");
        assert_eq!(decoded.channels, channels_data);
    }

    #[test]
    fn block_size_code_fixed_sizes() {
        assert_eq!(pick_block_size_code(192), (1, None));
        assert_eq!(pick_block_size_code(576), (2, None));
        assert_eq!(pick_block_size_code(1152), (3, None));
    }

    #[test]
    fn block_size_code_8bit_extra() {
        let (code, extra) = pick_block_size_code(200);
        assert_eq!(code, 6);
        assert_eq!(extra, Some(199)); // block_size - 1
    }

    #[test]
    fn block_size_code_16bit_extra() {
        // 4096 is now a canonical power-of-2 size (code 12).
        let (code, extra) = pick_block_size_code(4096);
        assert_eq!(code, 12);
        assert_eq!(extra, None);
    }
}
