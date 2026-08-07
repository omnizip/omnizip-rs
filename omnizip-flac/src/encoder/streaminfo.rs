//! STREAMINFO metadata block builder.
//!
//! Constructs the 34-byte STREAMINFO payload + the 4-byte metadata
//! block header that wraps it. The STREAMINFO block is mandatory and
//! must be the first metadata block in a FLAC stream.

#![forbid(unsafe_code)]

/// STREAMINFO payload size (bytes 0-33 of the metadata block body).
const STREAMINFO_SIZE: usize = 34;

/// Build the full STREAMINFO metadata block (header + payload) = 38 bytes.
///
/// `md5` should be the MD5 hash of the raw PCM audio data, or all zeros
/// if not computed.
#[must_use]
pub fn build_streaminfo_block(
    min_block_size: u16,
    max_block_size: u16,
    sample_rate: u32,
    channels: u8,
    bps: u8,
    total_samples: u64,
    md5: [u8; 16],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + STREAMINFO_SIZE);

    // Metadata block header (4 bytes):
    //   bit 0: last-metadata-block flag (we set this = 1).
    //   bits 1-7: block type (0 = STREAMINFO).
    out.push(0x80); // last=1, type=0
                    //   bits 8-31: length of payload in bytes = 34.
    out.extend_from_slice(&(STREAMINFO_SIZE as u32).to_be_bytes()[1..4]);

    // STREAMINFO payload (34 bytes).
    // Bytes 0-1: minimum block size (u16 BE).
    out.extend_from_slice(&min_block_size.to_be_bytes());
    // Bytes 2-3: maximum block size (u16 BE).
    out.extend_from_slice(&max_block_size.to_be_bytes());
    // Bytes 4-6: minimum frame size (u24 BE, 0 = unknown).
    out.extend_from_slice(&[0, 0, 0]);
    // Bytes 7-9: maximum frame size (u24 BE, 0 = unknown).
    out.extend_from_slice(&[0, 0, 0]);

    // Bytes 10-13 pack: 20-bit sample_rate | 3-bit (channels-1) |
    // 5-bit (bps-1) | 4 high bits of total_samples.
    // Total = 20 + 3 + 5 + 4 = 32 bits = 4 bytes.
    let sr = sample_rate & 0xFFFFF;
    let ch = u32::from(channels.saturating_sub(1) & 0x07);
    let bps_field = u32::from(bps.saturating_sub(1) & 0x1F);
    let total_hi = (total_samples >> 32) as u32 & 0x0F;
    let total_lo = total_samples as u32;
    let packed = (sr << 12) | (ch << 9) | (bps_field << 4) | total_hi;
    out.extend_from_slice(&packed.to_be_bytes());

    // Bytes 14-17: low 32 bits of total samples (u32 BE).
    out.extend_from_slice(&total_lo.to_be_bytes());

    // Bytes 18-33: MD5 of unencoded audio data.
    out.extend_from_slice(&md5);

    debug_assert_eq!(out.len(), 38);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaminfo::StreamInfo;

    #[test]
    fn build_then_parse_round_trips() {
        let md5 = [0xABu8; 16];
        let block = build_streaminfo_block(4096, 4096, 44_100, 2, 16, 1_000_000, md5);
        // First 4 bytes = header.
        assert_eq!(block[0], 0x80); // last=1, type=0
                                    // Bytes 1-3 = length = 34 = 0x000022.
        assert_eq!(&block[1..4], &[0, 0, 34]);

        // Parse the payload via existing decoder.
        let info = StreamInfo::parse(&block[4..]).expect("parse");
        assert_eq!(info.sample_rate, 44_100);
        assert_eq!(info.channel_count(), 2);
        assert_eq!(info.bps(), 16);
        assert_eq!(info.min_block_size, 4096);
        assert_eq!(info.max_block_size, 4096);
        assert_eq!(info.total_samples, 1_000_000);
        assert_eq!(info.md5, md5);
    }

    #[test]
    fn mono_8bit_round_trips() {
        let block = build_streaminfo_block(192, 192, 8_000, 1, 8, 100, [0u8; 16]);
        let info = StreamInfo::parse(&block[4..]).expect("parse");
        assert_eq!(info.sample_rate, 8_000);
        assert_eq!(info.channel_count(), 1);
        assert_eq!(info.bps(), 8);
    }
}
