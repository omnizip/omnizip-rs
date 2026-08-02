//! omnizip-flac — Pure-Rust FLAC audio codec.
//!
//! This crate provides:
//! - **PCM header parsers** for WAV and AIFF — immediately useful for
//!   extracting sample format parameters from audio files.
//! - **FLAC audio codec — decoder for all subframe types + raw PCM encoder.
//!   codec can be registered. The full FLAC encoder/decoder (LPC +
//!   Rice residual + CRC) is a large port from libFLAC and is being
//!   filled in incrementally.
//!
//! ## PCM header parsers
//!
//! The [`pcm_header`] module extracts [`PcmParams`] from WAV and AIFF
//! files. Consumers (e.g. LimniFS) use these to configure a FLAC
//! encoder without parsing the container format themselves.
//!
//! ## Wire format
//!
//! The current implementation stores PCM data verbatim with a
//! self-describing header. This round-trips correctly and is useful as
//! a wire format for PCM pipelines, but does NOT yet produce FLAC
//! bitstreams.
//!
//! ```text
//! +-------------------+  1 byte:  format marker (0 = raw PCM)
//! | format            |
//! +-------------------+  4 bytes LE: sample_rate
//! | sample_rate       |
//! +-------------------+  1 byte:  channels
//! | channels          |
//! +-------------------+  1 byte:  bits_per_sample
//! | bits_per_sample   |
//! +-------------------+  1 byte:  endianness (0 = LE, 1 = BE)
//! | endianness        |
//! +-------------------+  4 bytes LE: sample count
//! | sample_count      |
//! +-------------------+  variable: raw PCM data
//! | pcm_data          |
//! +-------------------+
//! ```

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_lossless)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

pub mod bitreader;
pub mod crc;
pub mod decoder;
pub mod encoder;
pub mod frame;
pub mod pcm_header;
pub mod rice;
pub mod streaminfo;
pub mod subframe;

pub use pcm_header::{Endianness, PcmParams};

/// FLAC codec id.
pub const FLAC_CODEC_ID: CodecId = CodecId::new(0x0012);

/// FLAC stream magic bytes ("fLaC").
const FLAC_MAGIC: [u8; 4] = *b"fLaC";

/// Compress audio data into a full FLAC stream (`fLaC` magic +
/// STREAMINFO + frames).
///
/// The output is a valid FLAC bitstream readable by libFLAC and the
/// built-in [`decoder`]. Each frame picks the cheapest subframe type
/// (CONSTANT / VERBATIM / FIXED order 0-4) per channel.
///
/// # Errors
///
/// Returns [`OmnizipError::EncodeFailed`] on configuration errors.
pub fn compress(input: &[u8], params: &PcmParams) -> Result<Vec<u8>, OmnizipError> {
    let bytes_per_sample = usize::from(params.bits_per_sample) / 8;
    let expected = params.sample_count as usize * usize::from(params.channels) * bytes_per_sample;
    if input.len() < expected {
        return Err(OmnizipError::EncodeFailed {
            codec: FLAC_CODEC_ID,
            reason: format!(
                "input {} bytes shorter than expected PCM payload {} bytes",
                input.len(),
                expected
            ),
        });
    }

    encoder::encode_stream(&input[..expected], params).map_err(|reason| OmnizipError::EncodeFailed {
        codec: FLAC_CODEC_ID,
        reason,
    })
}

/// Header size for a FLAC stream: 4-byte magic + 4-byte STREAMINFO
/// metadata block header + 34-byte STREAMINFO payload = 42 bytes.
#[allow(dead_code)]
const FLAC_HEADER_SIZE: usize = 42;

/// Decompress FLAC data. Expects a real FLAC bitstream (starts with
/// `fLaC` magic).
///
/// # Errors
///
/// Returns [`OmnizipError::DecodeFailed`] on malformed input.
pub fn decompress(compressed: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    if decoder::is_flac_stream(compressed) {
        return decoder::decode_stream(compressed).map_err(|reason| OmnizipError::DecodeFailed {
            codec: FLAC_CODEC_ID,
            reason,
        });
    }
    Err(OmnizipError::DecodeFailed {
        codec: FLAC_CODEC_ID,
        reason: "not a FLAC stream (missing fLaC magic)".into(),
    })
}

/// Extract PCM params from a FLAC stream's STREAMINFO block.
///
/// # Errors
///
/// Returns [`OmnizipError::DecodeFailed`] on malformed input.
pub fn extract_params(compressed: &[u8]) -> Result<PcmParams, OmnizipError> {
    if !decoder::is_flac_stream(compressed) {
        return Err(OmnizipError::DecodeFailed {
            codec: FLAC_CODEC_ID,
            reason: "not a FLAC stream".into(),
        });
    }
    // Parse STREAMINFO from bytes 8..42 (skip 4-byte magic + 4-byte
    // metadata block header = 8 bytes prefix).
    if compressed.len() < 42 {
        return Err(OmnizipError::DecodeFailed {
            codec: FLAC_CODEC_ID,
            reason: "stream too short for STREAMINFO".into(),
        });
    }
    let info = crate::streaminfo::StreamInfo::parse(&compressed[8..42]).ok_or_else(|| OmnizipError::DecodeFailed {
        codec: FLAC_CODEC_ID,
        reason: "invalid STREAMINFO".into(),
    })?;
    Ok(PcmParams {
        sample_rate: info.sample_rate,
        channels: info.channel_count(),
        bits_per_sample: info.bps(),
        endianness: Endianness::LittleEndian,
        sample_count: info.total_samples as u32,
    })
}

/// FLAC codec adapter. Produces real FLAC bitstreams (fLaC magic +
/// STREAMINFO + frames with CONSTANT/VERBATIM/FIXED subframes).
#[derive(Clone, Copy, Debug, Default)]
pub struct FlacCodec;

impl FlacCodec {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Codec for FlacCodec {
    fn id(&self) -> CodecId {
        FLAC_CODEC_ID
    }

    fn name(&self) -> &'static str {
        "flac"
    }

    fn compress(&self, plaintext: &[u8], _level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        // Default PCM params: 44.1 kHz stereo 16-bit LE.
        let params = PcmParams {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
            endianness: Endianness::LittleEndian,
            sample_count: (plaintext.len() as u32) / 4, // 2ch × 16-bit = 4 bytes/sample
        };
        compress(plaintext, &params)
    }

    fn decompress(&self, compressed: &[u8], _expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        decompress(compressed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn round_trip_sine_wave() {
        let pcm = mono_sine(440.0, 8_000, 192);
        let params = PcmParams {
            sample_rate: 8_000,
            channels: 1,
            bits_per_sample: 16,
            endianness: Endianness::LittleEndian,
            sample_count: 192,
        };
        let compressed = compress(&pcm, &params).expect("compress");
        assert_eq!(&compressed[..4], FLAC_MAGIC);
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, pcm);
    }

    #[test]
    fn codec_trait_round_trips() {
        let codec = FlacCodec::new();
        // FlacCodec::compress assumes 44.1 kHz stereo 16-bit LE, 4 bytes/sample.
        // Build a valid 8-frame stereo input (32 bytes).
        let input = vec![0u8; 32];
        let compressed = codec.compress(&input, CompressionLevel::default()).expect("compress");
        let decompressed = codec.decompress(&compressed, input.len() as u32).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn determinism() {
        let codec = FlacCodec::new();
        let input = vec![0u8; 64];
        let a = codec.compress(&input, CompressionLevel::default()).expect("compress");
        let b = codec.compress(&input, CompressionLevel::default()).expect("compress");
        assert_eq!(a, b);
    }

    #[test]
    fn dc_signal_compresses_well() {
        let pcm = vec![0u8; 192 * 2]; // 192 mono samples, 16-bit
        let params = PcmParams {
            sample_rate: 8_000,
            channels: 1,
            bits_per_sample: 16,
            endianness: Endianness::LittleEndian,
            sample_count: 192,
        };
        let compressed = compress(&pcm, &params).expect("compress");
        // CONSTANT subframe: should compress 384 bytes → well under 80.
        assert!(compressed.len() < 80, "DC signal compressed to {} bytes", compressed.len());
    }
}
