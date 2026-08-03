//! Pure-Rust LZ4 codec — wraps [`lz4_flex`] (the standard pure-Rust LZ4
//! implementation) behind the [`omnizip_codecs::Codec`] trait.
//!
//! Two variants are registered as separate codecs:
//!
//! - [`Lz4FastCodec`] (codec id `LZ4`): the standard fast encoder.
//!   Throughput > 1 GB/s. Moderate ratio.
//! - [`Lz4HcCodec`] (codec id `LZ4_HC`): the high-compression encoder.
//!   2–3x better ratio at the cost of 5–10x slower encode. Decode speed
//!   is identical (same format).
//!
//! Both use the same block format with a 4-byte LE original-size prefix.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

mod hc;

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// LZ4 fast codec. Wraps `lz4_flex::compress_prepend_size`.
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
        Ok(lz4_flex::compress_prepend_size(plaintext))
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        decompress_lz4(compressed, expected_len, CodecId::LZ4)
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
        // LZ4 HC: same wire format as fast LZ4 but with hash-chain match
        // finder + lazy parsing. Implemented in-house because lz4_flex
        // 0.11 doesn't ship HC.
        let compressed = hc::compress(plaintext);
        let mut out = Vec::with_capacity(4 + compressed.len());
        out.extend_from_slice(&(plaintext.len() as u32).to_le_bytes());
        out.extend_from_slice(&compressed);
        Ok(out)
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        decompress_lz4(compressed, expected_len, CodecId::LZ4_HC)
    }
}

fn decompress_lz4(
    compressed: &[u8],
    expected_len: u32,
    codec: CodecId,
) -> Result<Vec<u8>, OmnizipError> {
    let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
        codec,
        reason: format!("expected_len {expected_len} exceeds usize"),
    })?;
    let result = lz4_flex::decompress_size_prepended(compressed).map_err(|e| {
        OmnizipError::DecodeFailed {
            codec,
            reason: format!("lz4 decompress failed: {e}"),
        }
    })?;
    if result.len() != expected_us {
        return Err(OmnizipError::LengthMismatch {
            codec,
            expected: expected_len,
            actual: result.len(),
        });
    }
    Ok(result)
}

/// Compress using LZ4 frame format (compatible with `lz4 -d` CLI).
///
/// Uses `lz4_flex::frame::FrameEncoder` which produces standard LZ4
/// frames with magic number, descriptor, blocks, and end mark.
///
/// # Errors
///
/// Returns [`OmnizipError::EncodeFailed`] on internal failure.
pub fn compress_frame(plaintext: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    use std::io::Write;
    let mut encoder = lz4_flex::frame::FrameEncoder::new(Vec::new());
    encoder.write_all(plaintext).map_err(|e| OmnizipError::EncodeFailed {
        codec: CodecId::LZ4,
        reason: format!("lz4 frame encode failed: {e}"),
    })?;
    encoder.finish().map_err(|e| OmnizipError::EncodeFailed {
        codec: CodecId::LZ4,
        reason: format!("lz4 frame finalize failed: {e}"),
    })
}

/// Decompress an LZ4 frame (compatible with `lz4` CLI output).
///
/// # Errors
///
/// Returns [`OmnizipError::DecodeFailed`] on malformed frame.
pub fn decompress_frame(compressed: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    use std::io::Read;
    let mut decoder = lz4_flex::frame::FrameDecoder::new(compressed);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).map_err(|e| OmnizipError::DecodeFailed {
        codec: CodecId::LZ4,
        reason: format!("lz4 frame decode failed: {e}"),
    })?;
    Ok(output)
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
        // HC output must decode through the fast decoder (same format).
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

    /// Regression: HC must actually use the HC match finder and produce
    /// different output than fast. Prior to this fix, both codecs called
    /// `compress_prepend_size` and produced identical bytes.
    #[test]
    fn hc_produces_different_output_than_fast() {
        // Mixed text + binary input where the HC match finder's deeper
        // search finds longer/optimal matches than fast's first-match heuristic.
        let mut data: Vec<u8> = Vec::new();
        for i in 0..5_000u32 {
            data.extend_from_slice(b"the quick brown fox jumps over the lazy dog ");
            data.push((i & 0xFF) as u8);
        }
        let fast = Lz4FastCodec
            .compress(&data, CompressionLevel::default())
            .expect("fast");
        let hc = Lz4HcCodec
            .compress(&data, CompressionLevel::default())
            .expect("hc");
        // HC should find longer matches and produce smaller output.
        assert!(
            hc.len() < fast.len(),
            "HC must beat fast on repetitive text; hc={} fast={}",
            hc.len(),
            fast.len()
        );
    }
}
