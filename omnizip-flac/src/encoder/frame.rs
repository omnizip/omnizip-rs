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

/// Encode one frame of `interleaved_samples` into `writer`.
///
/// `frame_number` is the sequential frame index (0-based). The samples
/// are interleaved: `[ch0_s0, ch1_s0, ch0_s1, ch1_s1, ...]` for stereo.
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

    let header_start = writer.position();

    // Sync code + reserved + blocking strategy (fixed-block = 0).
    writer.write_bits(SYNC_CODE, 14);
    writer.write_bits(0, 1); // reserved
    writer.write_bits(0, 1); // fixed-block strategy

    // Block size code (4 bits). Pick the smallest encoding.
    // Codes 1-5 map to fixed sizes; 6 = 8-bit extra; 7 = 16-bit extra.
    let (bs_code, bs_extra): (u64, Option<u32>) = pick_block_size_code(block_size);
    writer.write_bits(bs_code, 4);

    // Sample rate code (4 bits). 0 = get from STREAMINFO.
    writer.write_bits(0, 4);

    // Channel assignment (4 bits). 0-7 = (n-1) independent channels.
    let channel_assign = (num_channels - 1) as u64;
    if channel_assign > 7 {
        return Err(format!(
            "unsupported channel count {num_channels} (max 8 independent)"
        ));
    }
    writer.write_bits(channel_assign, 4);

    // Sample size (3 bits). 0 = get from STREAMINFO.
    writer.write_bits(0, 3);

    // Reserved bit.
    writer.write_bits(0, 1);

    // UTF-8 coded frame number. For small numbers (< 0x80), it's a
    // single byte.
    write_utf8_coded(writer, u64::from(frame_number));

    // Optional block size extra bytes.
    if let Some(extra) = bs_extra {
        if extra <= u8::MAX as u32 {
            writer.write_bits(u64::from(extra), 8);
        } else {
            writer.write_bits(u64::from(extra), 16);
        }
    }

    // CRC-8 of header bytes so far.
    let header_end = writer.position();
    let crc8 = crate::crc::crc8(&writer.as_bytes()[header_start..header_end]);
    writer.write_bits(u64::from(crc8), 8);

    // Subframes.
    for chan in channels_data {
        subframe::encode_subframe(writer, chan, info.bps())?;
    }

    // Byte-align (pad with zero bits).
    writer.flush_byte_aligned();

    // CRC-16 of all frame bytes.
    let frame_end = writer.position();
    let crc16 = crate::crc::crc16(&writer.as_bytes()[header_start..frame_end]);
    writer.write_bits(u64::from(crc16 & 0xFF), 8);
    writer.write_bits(u64::from(crc16 >> 8), 8);

    Ok(())
}

/// Map a block size to its 4-bit code + optional extra-byte payload.
///
/// Returns `(code, Some(extra))` for codes 6-7, or `(code, None)` for
/// the fixed-size codes 1-5.
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
    // 8-bit extra: code 6, extra = block_size - 1 (1..=256).
    if block_size <= 256 {
        return (6, Some(block_size as u32 - 1));
    }
    // 16-bit extra: code 7, extra = block_size - 1 (1..=65536).
    (7, Some(block_size as u32 - 1))
}

/// Write a UTF-8 coded number. For values < 0x80, this is a single byte.
/// For larger values, uses the multi-byte form matching `read_utf8_coded`.
fn write_utf8_coded(writer: &mut BitWriter, value: u64) {
    if value < 0x80 {
        writer.write_bits(value, 8);
        return;
    }
    // Determine byte count.
    let mut nbytes = 1usize;
    let mut max_val = 0x1F_FFFFu64; // 3 bytes
    while value > max_val && nbytes < 6 {
        nbytes += 1;
        max_val = (max_val << 5) | 0x1F;
    }
    // First byte: nbytes leading 1-bits, then a 0-bit, then the high bits.
    let first_mask = (0xFFu8 << (7 - nbytes)) & 0xFE;
    let high_bits = (value >> (6 * nbytes)) as u8;
    writer.write_bits(u64::from(first_mask | high_bits), 8);
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
        let (code, extra) = pick_block_size_code(4096);
        assert_eq!(code, 7);
        assert_eq!(extra, Some(4095));
    }
}
