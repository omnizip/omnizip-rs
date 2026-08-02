//! PPMd8 codec: wraps the PPMd8 model in a container format and
//! implements the `Codec` trait.
//!
//! ## Container format
//!
//! ```text
//! +--------------------+  5 bytes: b"PPD8\0"
//! | magic              |
//! +--------------------+  1 byte:  max_order (2..=16)
//! | max_order          |
//! +--------------------+  4 bytes LE: uncompressed size (u32)
//! | uncompressed_size  |
//! +--------------------+  variable: arithmetic-coded bitstream
//! | bitstream          |
//! +--------------------+
//! ```
//!
//! The magic distinguishes PPMd8 streams from PPMd7's `b"PPMD\0"`.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::cast_precision_loss,
    clippy::doc_markdown
)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

use super::model::{ArithDecoder, ArithEncoder, Ppmd8Model};
use super::{Ppmd8Error, PPMD8_MAGIC};

/// Minimum context order (Ruby `MIN_ORDER`).
const MIN_ORDER: u8 = 2;
/// Maximum context order (Ruby `MAX_ORDER`).
const MAX_ORDER: u8 = 16;
/// Default context order when none specified (Ruby `DEFAULT_ORDER`).
pub const DEFAULT_ORDER: u8 = 6;

/// PPMd8 codec struct.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ppmd8Codec;

impl Ppmd8Codec {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Map a compression level to a max order. Higher levels use deeper
    /// contexts (more memory, better ratio).
    fn level_to_order(level: CompressionLevel) -> u8 {
        let l = level.as_u8();
        if l <= 2 {
            2
        } else if l <= 6 {
            4
        } else if l <= 12 {
            6
        } else {
            8
        }
    }

    fn validate_order(order: u8) -> Result<(), Ppmd8Error> {
        if (MIN_ORDER..=MAX_ORDER).contains(&order) {
            Ok(())
        } else {
            Err(Ppmd8Error::InvalidOrder(order))
        }
    }
}

impl Codec for Ppmd8Codec {
    fn id(&self) -> CodecId {
        CodecId::PPMD8
    }

    fn name(&self) -> &'static str {
        "ppmd8"
    }

    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        let order = Self::level_to_order(level);
        compress(plaintext, order).map_err(|e| OmnizipError::EncodeFailed {
            codec: self.id(),
            reason: e.to_string(),
        })
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let expected = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: self.id(),
            reason: format!("expected_len {expected_len} overflows usize"),
        })?;
        decompress(compressed, expected).map_err(|e| OmnizipError::Corrupt {
            codec: self.id(),
            reason: e.to_string(),
        })
    }
}

/// Compress `input` with the given `max_order` using the PPMd8 model.
///
/// # Errors
///
/// Returns [`Ppmd8Error::InvalidOrder`] if `max_order` is outside
/// `[2, 16]`.
pub fn compress(input: &[u8], max_order: u8) -> Result<Vec<u8>, Ppmd8Error> {
    Ppmd8Codec::validate_order(max_order)?;

    let mut out = Vec::with_capacity(input.len() / 2 + 16);
    out.extend_from_slice(PPMD8_MAGIC);
    out.push(max_order);
    let uncompressed_size =
        u32::try_from(input.len()).map_err(|_| Ppmd8Error::TooLarge(input.len()))?;
    out.extend_from_slice(&uncompressed_size.to_le_bytes());

    if input.is_empty() {
        return Ok(out);
    }

    let mut model = Ppmd8Model::default_for(usize::from(max_order));
    {
        let mut enc = ArithEncoder::new();
        model.encode_stream(&mut enc, input);
        enc.flush(&mut out);
    }
    Ok(out)
}

/// Decompress a PPMd8 container produced by [`compress`].
///
/// # Errors
///
/// Returns [`Ppmd8Error`] on structural problems or size mismatch.
pub fn decompress(compressed: &[u8], expected_len: usize) -> Result<Vec<u8>, Ppmd8Error> {
    if compressed.len() < 10 {
        return Err(Ppmd8Error::Corrupt("header too short".into()));
    }
    if &compressed[0..5] != PPMD8_MAGIC {
        return Err(Ppmd8Error::BadMagic);
    }
    let max_order = compressed[5];
    Ppmd8Codec::validate_order(max_order)?;

    let size = u32::from_le_bytes([compressed[6], compressed[7], compressed[8], compressed[9]]);
    let size = usize::try_from(size).map_err(|_| Ppmd8Error::Corrupt("size overflow".into()))?;
    if size != expected_len {
        return Err(Ppmd8Error::Corrupt(format!(
            "size mismatch: header says {size}, caller expects {expected_len}"
        )));
    }

    if size == 0 {
        return Ok(Vec::new());
    }

    let bitstream = &compressed[10..];
    let mut model = Ppmd8Model::default_for(usize::from(max_order));
    let mut dec = ArithDecoder::new(bitstream);
    Ok(model.decode_stream(&mut dec, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "The quick brown fox jumps over the lazy dog. \
        Pack my box with five dozen liquor jugs. \
        She sells seashells by the seashore. \
        Peter Piper picked a peck of pickled peppers. \
        How much wood would a woodchuck chuck if a woodchuck could chuck wood?";

    fn round_trip(input: &[u8], order: u8) {
        let compressed = compress(input, order).expect("compress");
        let out = decompress(&compressed, input.len()).expect("decompress");
        assert_eq!(
            out,
            input,
            "round-trip failed at order {order} (len={})",
            input.len()
        );
    }

    #[test]
    fn round_trip_text_order4() {
        round_trip(TEXT.as_bytes(), 4);
    }

    #[test]
    fn round_trip_text_order6() {
        round_trip(TEXT.as_bytes(), 6);
    }

    #[test]
    fn round_trip_empty() {
        let c = compress(b"", 4).expect("compress empty");
        assert_eq!(c.len(), 10);
        let d = decompress(&c, 0).expect("decompress empty");
        assert!(d.is_empty());
    }

    #[test]
    fn round_trip_single_byte() {
        round_trip(b"X", 2);
    }

    #[test]
    fn round_trip_long_text() {
        let big: Vec<u8> = TEXT.bytes().cycle().take(20_000).collect();
        round_trip(&big, 6);
    }

    #[test]
    fn round_trip_with_runs() {
        let mut data = Vec::new();
        data.extend_from_slice(b"intro ");
        data.extend_from_slice(&[0u8; 500]);
        data.extend_from_slice(b" middle ");
        data.extend_from_slice(&[b'Z'; 300]);
        data.extend_from_slice(b" outro");
        round_trip(&data, 4);
    }

    #[test]
    fn rejects_invalid_order() {
        assert!(compress(b"hi", 0).is_err());
        assert!(compress(b"hi", 1).is_err());
        assert!(compress(b"hi", 17).is_err());
        assert!(compress(b"hi", 2).is_ok());
        assert!(compress(b"hi", 16).is_ok());
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bad = vec![0u8; 20];
        bad[0..5].copy_from_slice(b"XXXXX");
        assert!(decompress(&bad, 5).is_err());
    }

    #[test]
    fn rejects_size_mismatch() {
        let c = compress(b"hello", 4).expect("compress");
        assert!(decompress(&c, 10).is_err());
    }

    #[test]
    fn codec_trait_round_trip() {
        let codec = Ppmd8Codec::new();
        let input = TEXT.as_bytes();
        let compressed = codec
            .compress(input, CompressionLevel::default())
            .expect("compress");
        let out = codec
            .decompress(&compressed, u32::try_from(input.len()).unwrap())
            .expect("decompress");
        assert_eq!(out, input);
    }

    #[test]
    fn codec_id_is_ppmd8() {
        let codec = Ppmd8Codec::new();
        assert_eq!(codec.id(), CodecId::PPMD8);
        assert_eq!(codec.id().as_u16(), 0x0009);
    }

    #[test]
    fn determinism() {
        let input = TEXT.as_bytes();
        let a = compress(input, 4).expect("a");
        let b = compress(input, 4).expect("b");
        assert_eq!(a, b, "non-deterministic output");
    }

    #[test]
    fn level_mapping_monotonic() {
        let o0 = Ppmd8Codec::level_to_order(CompressionLevel::new(0));
        let o6 = Ppmd8Codec::level_to_order(CompressionLevel::new(6));
        let o22 = Ppmd8Codec::level_to_order(CompressionLevel::new(22));
        assert!(o0 <= o6);
        assert!(o6 <= o22);
    }

    #[test]
    fn achieves_compression_on_text() {
        let big: Vec<u8> = TEXT.bytes().cycle().take(10_000).collect();
        let c = compress(&big, 6).expect("compress");
        assert!(
            c.len() < big.len(),
            "compressed {} vs original {}; no compression",
            c.len(),
            big.len()
        );
        let ratio = c.len() as f64 / big.len() as f64;
        eprintln!(
            "ppmd8 text ratio: {:.3} ({} -> {})",
            ratio,
            big.len(),
            c.len()
        );
        assert!(ratio < 0.70, "ratio {ratio:.3} worse than 0.70 bound");
    }
}
