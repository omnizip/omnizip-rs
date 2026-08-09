//! FLAC STREAMINFO metadata block parser.
//!
//! The STREAMINFO block is the mandatory first metadata block in a
//! FLAC stream. It contains the sample format parameters needed to
//! decode all subsequent audio frames.

#![forbid(unsafe_code)]

/// FLAC stream parameters from the STREAMINFO block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamInfo {
    /// Minimum block size (in samples) used in the stream.
    pub min_block_size: u32,
    /// Maximum block size (in samples) used in the stream.
    pub max_block_size: u32,
    /// Minimum frame size in bytes (0 if unknown).
    pub min_frame_size: u32,
    /// Maximum frame size in bytes (0 if unknown).
    pub max_frame_size: u32,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of channels minus 1 (0 = mono, 1 = stereo).
    pub channels: u8,
    /// Bits per sample minus 1.
    pub bits_per_sample: u8,
    /// Total sample count (0 if unknown).
    pub total_samples: u64,
    /// MD5 checksum of the unencoded audio data.
    pub md5: [u8; 16],
}

impl StreamInfo {
    /// Number of audio channels.
    #[must_use]
    pub fn channel_count(&self) -> u8 {
        self.channels + 1
    }

    /// Actual bits per sample.
    #[must_use]
    pub fn bps(&self) -> u8 {
        self.bits_per_sample + 1
    }

    /// Parse a 34-byte STREAMINFO block payload.
    ///
    /// # Errors
    ///
    /// Returns `None` if the data is too short for a STREAMINFO block.
    #[must_use]
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 34 {
            return None;
        }
        let min_block_size = u16::from_be_bytes([data[0], data[1]]) as u32;
        let max_block_size = u16::from_be_bytes([data[2], data[3]]) as u32;
        let min_frame_size = u32::from_be_bytes([0, data[4], data[5], data[6]]) >> 8;
        let max_frame_size = u32::from_be_bytes([0, data[7], data[8], data[9]]) >> 8;

        // Bytes 10-12: 20-bit sample_rate | 3-bit (channels-1) | 5-bit (bps-1).
        // The 20-bit sample_rate occupies the TOP 20 bits of the 24-bit
        // BE value from bytes 10-12.
        let raw24 = u32::from_be_bytes([0, data[10], data[11], data[12]]);
        let sample_rate = (raw24 >> 4) & 0xFFFFF;
        let channels = ((data[12] >> 1) & 0x07);
        let bps_high = data[12] & 0x01;
        let bps_low = data[13] >> 4;
        let bits_per_sample = (bps_high << 4) | bps_low;

        let total_samples_hi = u32::from(data[13] & 0x0F) as u64;
        let total_samples_lo = u32::from_be_bytes([data[14], data[15], data[16], data[17]]) as u64;
        let total_samples = (total_samples_hi << 32) | total_samples_lo;

        let mut md5 = [0u8; 16];
        md5.copy_from_slice(&data[18..34]);

        Some(Self {
            min_block_size,
            max_block_size,
            min_frame_size,
            max_frame_size,
            sample_rate,
            channels,
            bits_per_sample,
            total_samples,
            md5,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_streaminfo() {
        // 44.1kHz stereo 16-bit, block size 4096.
        let mut data = [0u8; 34];
        // min/max block size = 4096 = 0x1000.
        data[0] = 0x10;
        data[1] = 0x00;
        data[2] = 0x10;
        data[3] = 0x00;
        // Sample rate 44100 = 0xAC44. In STREAMINFO: bits 80..99 (20 bits).
        // data[10..12] = sample_rate >> 4. data[12] high nibble = sample_rate & 0xF.
        let sr = 44100u32;
        data[10] = (sr >> 12) as u8;
        data[11] = (sr >> 4) as u8;
        // channels-1 = 1 (stereo). bits 100..102.
        // bps-1 = 15 (16-bit). bits 103..107.
        // data[12] = (sr_low4 << 4) | (channels << 1) | bps_hi
        data[12] = ((sr & 0xF) as u8) << 4 | (1u8 << 1) | 0;
        data[13] = (15u8) << 4; // bps-1 = 15 → 0xF0.

        let info = StreamInfo::parse(&data).expect("parse");
        assert_eq!(info.sample_rate, 44100);
        assert_eq!(info.channel_count(), 2);
        assert_eq!(info.bps(), 16);
        assert_eq!(info.min_block_size, 4096);
    }

    #[test]
    fn parse_short_data_returns_none() {
        assert!(StreamInfo::parse(&[0u8; 10]).is_none());
    }
}
