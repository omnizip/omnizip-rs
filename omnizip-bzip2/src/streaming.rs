//! Streaming bzip2 decoder (omnizip-rs #712).
//!
//! Buffers compressed input and decodes on `finish`, handling the
//! concatenated multi-stream form the crate's [`streaming_encoder`]
//! emits (one independent `.bz2` member per chunk — `bzip2 -d`
//! semantics). Per-member incremental emission is a possible
//! follow-up: a stream's span is walkable the same way zstd's
//! `frame_span` is, but v1 keeps the decode-at-finish contract.
//!
//! [`streaming_encoder`]: crate::codec::streaming_encoder

#![forbid(unsafe_code)]

use omnizip_codecs::{CodecId, OmnizipError, StreamingDecoder};

use crate::bz2::decompress::decompress_multi_stream;

/// Streaming bzip2 decoder. Buffers input, decodes on finish.
pub struct Bzip2StreamingDecoder {
    buf: Vec<u8>,
    finished: bool,
}

impl Bzip2StreamingDecoder {
    /// Construct a fresh streaming decoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            finished: false,
        }
    }
}

impl Default for Bzip2StreamingDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingDecoder for Bzip2StreamingDecoder {
    fn write(&mut self, input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
        if self.finished {
            return Err(OmnizipError::DecodeFailed {
                codec: CodecId::BZIP2,
                reason: "write after finish".into(),
            });
        }
        self.buf.extend_from_slice(input);
        // v1 contract: no member-level framing walk yet — the whole
        // multi-stream file decodes on finish.
        Ok(Vec::new())
    }

    fn finish(self) -> Result<Vec<u8>, OmnizipError> {
        decompress_multi_stream(&self.buf)
    }
}

#[cfg(test)]
mod tests {
    use super::Bzip2StreamingDecoder;
    use crate::codec::streaming_encoder;
    use crate::Bzip2Codec;
    use omnizip_codecs::level::CompressionLevel;
    use omnizip_codecs::streaming::{StreamingDecoder, StreamingEncoder};
    use omnizip_codecs::Codec;

    #[test]
    fn round_trips_the_streaming_encoders_concat_output() {
        let input: Vec<u8> = (0..30_000u32).map(|i| (i % 249) as u8).collect();
        let mut e = streaming_encoder(CompressionLevel::default(), 8192);
        e.write(&input).unwrap();
        let compressed = e.finish().unwrap();

        let mut d = Bzip2StreamingDecoder::new();
        let mut i = 0;
        while i < compressed.len() {
            let n = 97.min(compressed.len() - i);
            d.write(&compressed[i..i + n]).unwrap();
            i += n;
        }
        assert_eq!(d.finish().unwrap(), input);
    }

    #[test]
    fn matches_one_shot_decode() {
        let input: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let compressed = Bzip2Codec
            .compress(&input, CompressionLevel::default())
            .unwrap();
        let mut d = Bzip2StreamingDecoder::new();
        d.write(&compressed[..compressed.len() / 2]).unwrap();
        d.write(&compressed[compressed.len() / 2..]).unwrap();
        assert_eq!(d.finish().unwrap(), input);
    }

    #[test]
    fn empty_stream_decodes_empty() {
        let mut e = streaming_encoder(CompressionLevel::default(), 8192);
        e.finish().unwrap();
        // The encoder errors on no data; feed a real empty member
        // produced by the one-shot instead.
        let compressed = Bzip2Codec
            .compress(&[], CompressionLevel::default())
            .unwrap();
        let mut d = Bzip2StreamingDecoder::new();
        d.write(&compressed).unwrap();
        assert!(d.finish().unwrap().is_empty());
    }

    #[test]
    fn trailing_garbage_is_an_error() {
        let compressed = Bzip2Codec
            .compress(b"some payload", CompressionLevel::default())
            .unwrap();
        let mut bad = compressed.clone();
        bad.extend_from_slice(b"JUNK");
        let mut d = Bzip2StreamingDecoder::new();
        d.write(&bad).unwrap();
        assert!(d.finish().is_err());
    }
}
