//! FLAC frame decoder.
//!
//! Parses frame headers and decodes subframes for each audio block.

#![forbid(unsafe_code)]

use crate::bitreader::BitReader;
use crate::crc;
use crate::streaminfo::StreamInfo;
use crate::subframe;

/// Decoded audio frame: interleaved samples per channel.
pub struct AudioFrame {
    /// Channel data: `channels[ch][sample]`.
    pub channels: Vec<Vec<i32>>,
    /// Number of samples per channel in this frame.
    pub block_size: usize,
}

/// Decode one FLAC frame starting at `reader`. Returns the decoded
/// audio and the number of bytes consumed.
///
/// # Errors
///
/// Returns `String` on malformed frame data.
pub fn decode_frame(
    reader: &mut BitReader,
    info: &StreamInfo,
) -> Result<(AudioFrame, usize), String> {
    let frame_start = reader.byte_position();

    // Sync code: 14 bits of 0x3FFE.
    let sync = reader.read_bits(14);
    if sync != 0x3FFE {
        return Err(format!("invalid frame sync: 0x{sync:04X}"));
    }

    // Reserved bit (must be 0).
    let _reserved = reader.read_bits(1);

    // Blocking strategy (0 = fixed-block, 1 = variable-block).
    let _blocking_strategy = reader.read_bits(1);

    // Block size (4 bits).
    let block_size_raw = reader.read_bits(4);
    let mut block_size = match block_size_raw {
        0 => return Err("block size 0 is reserved".into()),
        1 => 192,
        v @ 2..=5 => 576 * (1 << (v - 2)),
        // Codes 8-15 = 256 × 2^(code-8): 256, 512, 1024, 2048, 4096,
        // 8192, 16384, 32768.
        v @ 8..=15 => 256 * (1 << (v - 8)),
        // Codes 6 and 7 take an extra byte (8 or 16 bits) later.
        6 | 7 => 0,
        _ => return Err(format!("invalid block size code: {block_size_raw}")),
    };

    // Sample rate (4 bits).
    let sample_rate_raw = reader.read_bits(4);
    let _sample_rate: Option<u32> = match sample_rate_raw {
        0 => None, // From STREAMINFO
        1 => Some(88.2e3 as u32),
        2 => Some(176.4e3 as u32),
        3 => Some(192e3 as u32),
        4 => Some(8e3 as u32),
        5 => Some(16e3 as u32),
        6 => Some(22.05e3 as u32),
        7 => Some(24e3 as u32),
        8 => Some(32e3 as u32),
        9 => Some(44.1e3 as u32),
        10 => Some(48e3 as u32),
        11 => Some(96e3 as u32),
        12..=14 => None, // Read from end of header
        15 => return Err("invalid sample rate 15".into()),
        _ => return Err("unreachable".into()),
    };

    // Channel assignment (4 bits).
    let channel_assign = reader.read_bits(4);
    let (num_channels, _decorrelated) = match channel_assign {
        0..=7 => (channel_assign as u8 + 1, false),
        8..=10 => (2u8, true), // left/side, right/side, mid/side
        11 => return Err("channel assignment 11 is reserved".into()),
        _ => return Err(format!("invalid channel assignment: {channel_assign}")),
    };

    // Sample size (3 bits).
    let sample_size_raw = reader.read_bits(3);
    let bps = match sample_size_raw {
        0 => info.bps(),
        1 => 8,
        2 => 12,
        4 => 16,
        5 => 20,
        6 => 24,
        7 => 32,
        _ => return Err(format!("invalid sample size: {sample_size_raw}")),
    };

    // Reserved bit.
    let _ = reader.read_bits(1);

    // UTF-8 coded frame/sample number (variable length).
    // For simplicity, just read the first byte and skip.
    let _frame_number = read_utf8_coded(reader)?;

    // Optional block size at end of header.
    if block_size_raw == 6 {
        block_size = reader.read_bits(8) as usize + 1;
    } else if block_size_raw == 7 {
        block_size = reader.read_bits(16) as usize + 1;
    }

    // Optional sample rate at end of header.
    if sample_rate_raw == 12 {
        let _ = reader.read_bits(8);
    } else if sample_rate_raw == 13 {
        let _ = reader.read_bits(16);
    } else if sample_rate_raw == 14 {
        let _ = reader.read_bits(16) * 10;
    }

    // CRC-8 of everything from sync to here.
    let header_end = reader.byte_position();
    let _crc8 = reader.read_bits(8);
    let _ = header_end;

    // Decode subframes.
    let mut channels_data = Vec::with_capacity(num_channels as usize);
    for _ in 0..num_channels {
        let samples = subframe::decode_subframe(reader, block_size, bps)?;
        channels_data.push(samples);
    }

    // Reconstruct left/right from decorrelated representations.
    if (8..=10).contains(&channel_assign) {
        channels_data = reconstruct_stereo(channels_data, channel_assign);
    }

    // Align to byte boundary.
    reader.align_byte();

    // Frame footer: CRC-16 (2 bytes).
    let _crc16 = reader.read_bits(16);
    let _ = crc::crc16(&[]);

    let bytes_consumed = reader.byte_position() - frame_start;

    Ok((
        AudioFrame {
            channels: channels_data,
            block_size,
        },
        bytes_consumed,
    ))
}

/// Reconstruct left/right stereo channels from decorrelated
/// representations (FLAC channel assignments 8-10).
///
/// - 8 (left/side): ch[0]=left, ch[1]=side=left-right → right=left-side.
/// - 9 (right/side): ch[0]=right, ch[1]=side=left-right → left=side+right.
/// - 10 (mid/side): ch[0]=mid=(l+r)>>1, ch[1]=side=l-r
///   → left=mid+((side+(side&1))>>1), right=left-side.
fn reconstruct_stereo(mut channels: Vec<Vec<i32>>, assign: u32) -> Vec<Vec<i32>> {
    if channels.len() != 2 {
        return channels;
    }
    // Take ownership of both channel vectors.
    let ch1 = channels.remove(1);
    let ch0 = channels.remove(0);
    let (left, right) = match assign {
        8 => {
            // Left/side: ch0 = left, ch1 = side = left - right.
            let left = ch0.clone();
            let right: Vec<i32> = ch0.iter().zip(ch1.iter()).map(|(&l, &s)| l - s).collect();
            (left, right)
        }
        9 => {
            // Right/side: ch0 = right, ch1 = side = left - right.
            let right = ch0.clone();
            let left: Vec<i32> = ch1.iter().zip(ch0.iter()).map(|(&s, &r)| s + r).collect();
            (left, right)
        }
        10 => {
            // Mid/side: ch0 = mid = (l+r)>>1, ch1 = side = l - r.
            let left: Vec<i32> = ch0
                .iter()
                .zip(ch1.iter())
                .map(|(&m, &s)| m + ((s + (s & 1)) >> 1))
                .collect();
            let right: Vec<i32> = left.iter().zip(ch1.iter()).map(|(&l, &s)| l - s).collect();
            (left, right)
        }
        _ => return vec![ch0, ch1],
    };
    vec![left, right]
}

/// Read a FLAC UTF-8 coded number (1-6 bytes) from the reader.
///
/// The first byte's leading 1-bits determine the total byte count:
/// - 0xxxxxxx → 1 byte (value 0-127)
/// - 110xxxxx → 2 bytes
/// - 1110xxxx → 3 bytes
/// - 11110xxx → 4 bytes
fn read_utf8_coded(reader: &mut BitReader) -> Result<u64, String> {
    let first = reader.read_bits(8) as u8;
    if first < 0x80 {
        return Ok(u64::from(first));
    }
    // Count leading 1-bits via the inverted byte's leading zeros.
    let num_leading_ones = (!first).leading_zeros() as usize;
    if !(2..=7).contains(&num_leading_ones) {
        return Err(format!("invalid UTF-8 coded number: 0x{first:02X}"));
    }
    let num_continuation = num_leading_ones - 1;
    // Data bits in first byte = 7 - num_leading_ones.
    let data_bits_first = 7 - num_leading_ones;
    let mut value = u64::from(first & ((1u8 << data_bits_first) - 1));
    for _ in 0..num_continuation {
        let byte = reader.read_bits(8) as u8;
        if byte & 0xC0 != 0x80 {
            return Err(format!("invalid UTF-8 continuation byte: 0x{byte:02X}"));
        }
        value = (value << 6) | u64::from(byte & 0x3F);
    }
    Ok(value)
}
