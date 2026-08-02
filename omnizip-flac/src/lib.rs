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

/// Format marker for raw PCM passthrough.
const FORMAT_RAW_PCM: u8 = 0;

/// Compress audio data. The current implementation stores the PCM
/// data with a self-describing header (see crate-level docs). A full
/// FLAC VERBATIM encoder + full decoder for CONSTANT/VERBATIM/FIXED/LPC subframes.
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

    let mut out = Vec::with_capacity(12 + input.len());
    out.push(FORMAT_RAW_PCM);
    out.extend_from_slice(&params.sample_rate.to_le_bytes());
    out.push(params.channels);
    out.push(params.bits_per_sample);
    out.push(match params.endianness {
        Endianness::LittleEndian => 0,
        Endianness::BigEndian => 1,
    });
    out.extend_from_slice(&params.sample_count.to_le_bytes());
    out.extend_from_slice(&input[..expected]);
    Ok(out)
}

/// Header size for the raw-PCM container.
const HEADER_SIZE: usize = 12;

/// Decompress FLAC data. Detects whether the input is a FLAC stream
/// (starts with `fLaC` magic) or a raw-PCM container.
///
/// # Errors
///
/// Returns [`OmnizipError::DecodeFailed`] on malformed input.
pub fn decompress(compressed: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    // Check for FLAC stream magic.
    if decoder::is_flac_stream(compressed) {
        return decoder::decode_stream(compressed).map_err(|reason| OmnizipError::DecodeFailed {
            codec: FLAC_CODEC_ID,
            reason,
        });
    }

    // Raw-PCM container format.
    if compressed.len() < HEADER_SIZE {
        return Err(OmnizipError::DecodeFailed {
            codec: FLAC_CODEC_ID,
            reason: "header too short".into(),
        });
    }
    let format = compressed[0];
    if format != FORMAT_RAW_PCM {
        return Err(OmnizipError::DecodeFailed {
            codec: FLAC_CODEC_ID,
            reason: format!("unsupported format marker: {format}"),
        });
    }

    Ok(compressed[HEADER_SIZE..].to_vec())
}

/// Extract PCM params from `compressed` (must have been produced by
/// [`compress`]). Useful when the consumer needs the sample format to
/// route the data downstream.
///
/// # Errors
///
/// Returns [`OmnizipError::DecodeFailed`] on malformed input.
pub fn extract_params(compressed: &[u8]) -> Result<PcmParams, OmnizipError> {
    if compressed.len() < HEADER_SIZE {
        return Err(OmnizipError::DecodeFailed {
            codec: FLAC_CODEC_ID,
            reason: "header too short".into(),
        });
    }
    Ok(PcmParams {
        sample_rate: u32::from_le_bytes([compressed[1], compressed[2], compressed[3], compressed[4]]),
        channels: compressed[5],
        bits_per_sample: compressed[6],
        endianness: match compressed[7] {
            1 => Endianness::BigEndian,
            _ => Endianness::LittleEndian,
        },
        sample_count: u32::from_le_bytes([compressed[8], compressed[9], compressed[10], compressed[11]]),
    })
}

/// FLAC codec adapter. Stores PCM data with a self-describing header.
/// Full FLAC bitstream support is planned.
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

    #[test]
    fn round_trip_raw_pcm() {
        let pcm = vec![0u8; 1000];
        let params = PcmParams {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
            endianness: Endianness::LittleEndian,
            sample_count: 250, // 1000 bytes / 4 bytes per sample
        };
        let compressed = compress(&pcm, &params).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, pcm);
    }

    #[test]
    fn codec_trait_round_trips() {
        let codec = FlacCodec::new();
        let input = vec![0xAA; 800];
        let compressed = codec.compress(&input, CompressionLevel::default()).expect("compress");
        let decompressed = codec.decompress(&compressed, input.len() as u32).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn determinism() {
        let codec = FlacCodec::new();
        let input = vec![0x55; 400];
        let a = codec.compress(&input, CompressionLevel::default()).expect("compress");
        let b = codec.compress(&input, CompressionLevel::default()).expect("compress");
        assert_eq!(a, b);
    }
}
