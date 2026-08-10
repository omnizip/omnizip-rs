//! Pure-Rust LZ4 codec — in-house block + frame encoder + decoder.
//!
//! Two variants are registered as separate codecs:
//!
//! - [`Lz4FastCodec`] (codec id `LZ4`): fast single-probe encoder.
//!   Throughput > 500 MB/s. Moderate ratio.
//! - [`Lz4HcCodec`] (codec id `LZ4_HC`): high-compression encoder with
//!   hash-chain match finder + lazy parsing (see [`hc`]). 2-3× better
//!   ratio at the cost of slower encode.
//!
//! Both use the same LZ4 block format with a 4-byte LE original-size
//! prefix. The in-house encoder + decoder are implemented from spec in
//! [`block`] and [`frame`].

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod block;
pub mod frame;
mod hc;
pub mod streaming;

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// LZ4 fast codec. Uses the in-house single-probe encoder.
pub struct Lz4FastCodec;

/// LZ4 high-compression codec. Uses an in-house hash-chain match
/// finder + lazy parsing (see [`hc`]). Same decode path as
/// [`Lz4FastCodec`] — produces LZ4 block-format bytes that any
/// LZ4 decoder can read.
pub struct Lz4HcCodec;

impl Codec for Lz4FastCodec {
    fn id(&self) -> CodecId {
        CodecId::LZ4
    }
    fn name(&self) -> &'static str {
        "lz4"
    }
    fn compress(
        &self,
        plaintext: &[u8],
        _level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        let compressed = block::compress_block(plaintext);
        let mut out = Vec::with_capacity(4 + compressed.len());
        out.extend_from_slice(&(plaintext.len() as u32).to_le_bytes());
        out.extend_from_slice(&compressed);
        Ok(out)
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        decompress_lz4(compressed, expected_len, CodecId::LZ4)
    }

    fn default_fast_level(&self) -> u8 {
        1
    }
    fn default_balanced_level(&self) -> u8 {
        1
    }
    fn default_max_ratio_level(&self) -> u8 {
        1
    }

    fn capabilities(&self) -> omnizip_codecs::Capabilities {
        omnizip_codecs::Capabilities {
            min_level: 1,
            max_level: 1,
            streaming: true, // Lz4StreamingEncoder/Decoder landed
            parallel_batch: true,
            has_static_dictionary: false,
            content_type_aware: false,
            approx_throughput_mbps: 500,
        }
    }
}

impl Codec for Lz4HcCodec {
    fn id(&self) -> CodecId {
        CodecId::LZ4_HC
    }
    fn name(&self) -> &'static str {
        "lz4-hc"
    }
    fn compress(
        &self,
        plaintext: &[u8],
        _level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        let compressed = hc::compress(plaintext);
        let mut out = Vec::with_capacity(4 + compressed.len());
        out.extend_from_slice(&(plaintext.len() as u32).to_le_bytes());
        out.extend_from_slice(&compressed);
        Ok(out)
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        decompress_lz4(compressed, expected_len, CodecId::LZ4_HC)
    }

    fn default_fast_level(&self) -> u8 {
        4
    }
    fn default_balanced_level(&self) -> u8 {
        9
    }
    fn default_max_ratio_level(&self) -> u8 {
        12
    }

    fn capabilities(&self) -> omnizip_codecs::Capabilities {
        omnizip_codecs::Capabilities {
            min_level: 1,
            max_level: 12,
            streaming: false, // HC mode is one-shot
            parallel_batch: true,
            has_static_dictionary: false,
            content_type_aware: false,
            approx_throughput_mbps: 200,
        }
    }
}

/// Decompress a size-prepended LZ4 block (4-byte LE size + block data).
fn decompress_lz4(
    compressed: &[u8],
    expected_len: u32,
    codec: CodecId,
) -> Result<Vec<u8>, OmnizipError> {
    if compressed.len() < 4 {
        return Err(OmnizipError::Corrupt {
            codec,
            reason: "input too short for size prefix".into(),
        });
    }
    let stored_len =
        u32::from_le_bytes([compressed[0], compressed[1], compressed[2], compressed[3]]) as usize;
    let block_data = &compressed[4..];
    let decoded = block::decompress_block(block_data, stored_len).map_err(|reason| {
        OmnizipError::DecodeFailed {
            codec,
            reason: format!("lz4 block decode failed: {reason}"),
        }
    })?;

    let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
        codec,
        reason: format!("expected_len {expected_len} exceeds usize"),
    })?;
    if decoded.len() != expected_us {
        return Err(OmnizipError::LengthMismatch {
            codec,
            expected: expected_len,
            actual: decoded.len(),
        });
    }
    Ok(decoded)
}

/// Compress using LZ4 frame format (compatible with `lz4 -d` CLI).
///
/// Uses the in-house frame encoder from [`frame`].
///
/// # Errors
///
/// Currently infallible; returns `Ok` always.
pub fn compress_frame(plaintext: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    Ok(frame::compress_frame(plaintext))
}

/// Decompress an LZ4 frame (compatible with `lz4` CLI output).
///
/// # Errors
///
/// Returns [`OmnizipError::DecodeFailed`] on malformed frame.
pub fn decompress_frame(compressed: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    frame::decompress_frame(compressed).map_err(|reason| OmnizipError::DecodeFailed {
        codec: CodecId::LZ4,
        reason: format!("lz4 frame decode failed: {reason}"),
    })
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn fast_round_trips_text() {
        let data = b"Lorem ipsum dolor sit amet. ".repeat(100);
        let compressed = Lz4FastCodec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        let decompressed = Lz4FastCodec
            .decompress(&compressed, data.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, data);
    }

    #[test]
    fn hc_round_trips_text() {
        let data = b"Lorem ipsum dolor sit amet. ".repeat(100);
        let compressed = Lz4HcCodec
            .compress(&data, CompressionLevel::default())
            .expect("compress");
        let decompressed = Lz4HcCodec
            .decompress(&compressed, data.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, data);
    }

    #[test]
    fn fast_and_hc_share_decoder_format() {
        let data = b"The quick brown fox. ".repeat(1000);
        let fast_compressed = Lz4FastCodec
            .compress(&data, CompressionLevel::default())
            .expect("fast compress");
        let hc_compressed = Lz4HcCodec
            .compress(&data, CompressionLevel::default())
            .expect("hc compress");
        let from_fast = Lz4FastCodec
            .decompress(&hc_compressed, data.len() as u32)
            .expect("cross-decode fast from hc");
        let from_hc = Lz4HcCodec
            .decompress(&fast_compressed, data.len() as u32)
            .expect("cross-decode hc from fast");
        assert_eq!(from_fast, data);
        assert_eq!(from_hc, data);
    }

    #[test]
    fn compresses_repetitive_data() {
        let data = vec![0x41u8; 10_000];
        let fast = Lz4FastCodec
            .compress(&data, CompressionLevel::default())
            .expect("fast");
        assert!(fast.len() < data.len());
    }

    #[test]
    fn hc_produces_different_output_than_fast() {
        let data: Vec<u8> = (0..10_000)
            .map(|i| {
                if i % 100 < 50 {
                    (i % 26 + b'a' as i32) as u8
                } else {
                    (i % 256) as u8
                }
            })
            .collect();
        let fast = Lz4FastCodec
            .compress(&data, CompressionLevel::default())
            .expect("fast");
        let hc = Lz4HcCodec
            .compress(&data, CompressionLevel::default())
            .expect("hc");
        assert_ne!(fast, hc, "HC must produce different output");
    }

    #[test]
    fn frame_round_trip() {
        let data = b"the quick brown fox jumps over the lazy dog. ".repeat(10);
        let compressed = compress_frame(&data).expect("frame compress");
        let decompressed = decompress_frame(&compressed).expect("frame decompress");
        assert_eq!(decompressed.as_slice(), data.as_slice());
    }
}
