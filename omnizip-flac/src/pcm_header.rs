//! PCM header parsers for WAV and AIFF container formats.
//!
//! These extract the sample format parameters needed to configure a
//! FLAC encoder. The parser is tightly coupled to the codec's
//! parameter format, so it lives in omnizip-flac (not in the consumer).

#![forbid(unsafe_code)]

/// Byte order of the PCM sample data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endianness {
    LittleEndian,
    BigEndian,
}

/// PCM parameters extracted from a WAV/AIFF header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PcmParams {
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    pub endianness: Endianness,
    /// Total sample frames (1 frame = 1 sample per channel).
    pub sample_count: u32,
}

impl PcmParams {
    /// Bytes per sample frame: `channels × bits_per_sample / 8`.
    #[must_use]
    pub fn bytes_per_frame(self) -> usize {
        usize::from(self.channels) * usize::from(self.bits_per_sample) / 8
    }

    /// Total PCM payload bytes: `sample_count × bytes_per_frame`.
    #[must_use]
    pub fn total_bytes(self) -> usize {
        self.sample_count as usize * self.bytes_per_frame()
    }
}

/// Parse a WAV (RIFF/WAVE) file and extract PCM parameters.
///
/// Returns `None` if the input is not a valid WAV file or uses a
/// compressed codec (only PCM = format code 1 is supported).
#[must_use]
pub fn parse_wav(bytes: &[u8]) -> Option<PcmParams> {
    // RIFF header: "RIFF" + 4 bytes size + "WAVE"
    if bytes.len() < 12 {
        return None;
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }

    let mut pos = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (format, channels, sample_rate, bits_per_sample)
    let mut data_len: Option<u32> = None;

    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]);
        pos += 8;

        if chunk_id == b"fmt " {
            if chunk_size < 16 || pos + 16 > bytes.len() {
                return None;
            }
            let format_code = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]);
            let channels = u16::from_le_bytes([bytes[pos + 2], bytes[pos + 3]]);
            let sample_rate = u32::from_le_bytes([
                bytes[pos + 4],
                bytes[pos + 5],
                bytes[pos + 6],
                bytes[pos + 7],
            ]);
            let bits_per_sample = u16::from_le_bytes([bytes[pos + 14], bytes[pos + 15]]);
            // Only PCM (format code 1) is supported.
            if format_code != 1 {
                return None;
            }
            fmt = Some((format_code, channels, sample_rate, bits_per_sample));
        } else if chunk_id == b"data" {
            data_len = Some(chunk_size);
        }

        // Chunks are padded to even length.
        pos += chunk_size as usize;
        if chunk_size & 1 != 0 {
            pos += 1;
        }
    }

    let (_, channels, sample_rate, bits_per_sample) = fmt?;
    let data_len = data_len?;
    let bytes_per_frame = usize::from(channels) * (bits_per_sample as usize) / 8;
    let sample_count = if bytes_per_frame > 0 {
        data_len as usize / bytes_per_frame
    } else {
        0
    };

    Some(PcmParams {
        sample_rate,
        channels: channels as u8,
        bits_per_sample: bits_per_sample as u8,
        endianness: Endianness::LittleEndian,
        sample_count: sample_count as u32,
    })
}

/// Parse an AIFF (Audio Interchange File Format) file and extract PCM
/// parameters.
///
/// Returns `None` if the input is not a valid AIFF/AIFF-C file or
/// uses a compressed codec.
#[must_use]
pub fn parse_aiff(bytes: &[u8]) -> Option<PcmParams> {
    // FORM header: "FORM" + 4 bytes size + "AIFF" or "AIFC"
    if bytes.len() < 12 {
        return None;
    }
    if &bytes[0..4] != b"FORM" {
        return None;
    }
    let form_type = &bytes[8..12];
    let is_aiff = form_type == b"AIFF";
    let is_aifc = form_type == b"AIFC";
    if !is_aiff && !is_aifc {
        return None;
    }

    let mut pos = 12;
    let mut comm: Option<(u16, u64, u16, f64)> = None; // (channels, frames, bits, sample_rate_ext)
    let mut ssnd_len: Option<u32> = None;

    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let chunk_size = u32::from_be_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]);
        pos += 8;

        if chunk_id == b"COMM" {
            if chunk_size < 18 || pos + 18 > bytes.len() {
                return None;
            }
            let channels = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]);
            let frames = u32::from_be_bytes([
                bytes[pos + 2],
                bytes[pos + 3],
                bytes[pos + 4],
                bytes[pos + 5],
            ]);
            let bits_per_sample = u16::from_be_bytes([bytes[pos + 6], bytes[pos + 7]]);
            // 80-bit IEEE 754 extended float sample rate (big-endian).
            let sample_rate = parse_extended_float(&bytes[pos + 8..pos + 18])?;
            comm = Some((channels, u64::from(frames), bits_per_sample, sample_rate));

            // For AIFC, skip the compression type (4 bytes after COMM).
            if is_aifc && chunk_size >= 22 && pos + 22 <= bytes.len() {
                let compression_type = &bytes[pos + 18..pos + 22];
                // Only uncompressed PCM ("NONE" or "twos") is supported.
                if compression_type != b"NONE" && compression_type != b"twos" {
                    return None;
                }
            }
        } else if chunk_id == b"SSND" {
            ssnd_len = Some(chunk_size);
        }

        pos += chunk_size as usize;
        if chunk_size & 1 != 0 {
            pos += 1;
        }
    }

    let (channels, frames, bits_per_sample, sample_rate_f) = comm?;
    let _ = ssnd_len;

    Some(PcmParams {
        sample_rate: sample_rate_f.round() as u32,
        channels: channels as u8,
        bits_per_sample: bits_per_sample as u8,
        endianness: Endianness::BigEndian,
        sample_count: frames as u32,
    })
}

/// Parse a 10-byte big-endian IEEE 754 extended float (80-bit).
/// Returns None on invalid exponent.
fn parse_extended_float(bytes: &[u8]) -> Option<f64> {
    if bytes.len() < 10 {
        return None;
    }
    let sign_bit = (bytes[0] & 0x80) != 0;
    let exponent = u16::from_be_bytes([bytes[0] & 0x7F, bytes[1]]);
    let mantissa_hi = u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
    let mantissa_lo = u32::from_be_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);

    if exponent == 0 && mantissa_hi == 0 && mantissa_lo == 0 {
        return Some(0.0);
    }
    if exponent == 0x7FFF {
        return None; // Inf/NaN
    }

    // 64-bit mantissa: high 32 + low 32, with implicit integer bit.
    let mantissa = (u64::from(mantissa_hi) << 32) | u64::from(mantissa_lo);
    let bias = 16_383i32;
    let unbiased = i32::from(exponent) - bias;
    // value = mantissa × 2^(unbiased - 63)
    let value = (mantissa as f64) * (2f64).powi(unbiased - 63);
    Some(if sign_bit { -value } else { value })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_wav() {
        // Minimal WAV: 44-byte header, 4 bytes of data (1 sample, 16-bit mono).
        let wav: Vec<u8> = vec![
            b'R', b'I', b'F', b'F', // "RIFF"
            0x24, 0x00, 0x00, 0x00, // size = 36
            b'W', b'A', b'V', b'E', // "WAVE"
            b'f', b'm', b't', b' ', // "fmt "
            16, 0, 0, 0, // chunk size = 16
            1, 0, // PCM format
            1, 0, // mono
            0x44, 0xAC, 0x00, 0x00, // 44100 Hz
            0x88, 0x58, 0x01, 0x00, // byte rate
            2, 0, // block align
            16, 0, // 16 bits
            b'd', b'a', b't', b'a', // "data"
            4, 0, 0, 0, // 4 bytes
            0, 0, 0, 0, // PCM data
        ];
        let params = parse_wav(&wav).expect("parse");
        assert_eq!(params.sample_rate, 44100);
        assert_eq!(params.channels, 1);
        assert_eq!(params.bits_per_sample, 16);
        assert_eq!(params.endianness, Endianness::LittleEndian);
        assert_eq!(params.sample_count, 2); // 4 bytes / 2 bytes per frame
    }

    #[test]
    fn parse_invalid_wav_returns_none() {
        assert!(parse_wav(b"not a wav").is_none());
        assert!(parse_wav(b"RIFF\x00\x00\x00\x00XXXX").is_none());
    }

    #[test]
    fn parse_stereo_wav() {
        // Stereo 48kHz 16-bit, 8 bytes of data = 1 sample frame.
        let wav: Vec<u8> = vec![
            b'R', b'I', b'F', b'F', 0x00, 0x00, 0x00, 0x00, b'W', b'A', b'V', b'E', b'f', b'm',
            b't', b' ', 16, 0, 0, 0, 1, 0, // PCM format
            2, 0, // stereo
            0x80, 0xBB, 0x00, 0x00, // 48000 Hz
            0x00, 0xEE, 0x02, 0x00, // byte rate
            4, 0, // block align
            16, 0, // 16 bits
            b'd', b'a', b't', b'a', 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let params = parse_wav(&wav).expect("parse");
        assert_eq!(params.sample_rate, 48000);
        assert_eq!(params.channels, 2);
        assert_eq!(params.sample_count, 2); // 8 bytes / 4 bytes per frame
    }

    #[test]
    fn extended_float_44100() {
        // 44100 Hz as 80-bit IEEE 754 extended float (big-endian).
        // 44100 = 1.010110001000100 × 2^15
        // exponent = bias(16383) + 15 = 16398 = 0x400E
        // mantissa = 0xAC44000000000000 (explicit integer bit at bit 63)
        let bytes: [u8; 10] = [0x40, 0x0E, 0xAC, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let value = parse_extended_float(&bytes).expect("parse");
        assert!((value - 44100.0).abs() < 1.0, "got {value}");
    }

    #[test]
    fn pcm_params_bytes_per_frame() {
        let params = PcmParams {
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
            endianness: Endianness::LittleEndian,
            sample_count: 100,
        };
        assert_eq!(params.bytes_per_frame(), 4);
        assert_eq!(params.total_bytes(), 400);
    }
}
