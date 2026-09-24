//! Streaming bzip2 decoder (omnizip-rs #712).
//!
//! Handles the concatenated multi-stream form the crate's
//! [`streaming_encoder`] emits (one independent `.bz2` member per
//! chunk — `bzip2 -d` semantics), emitting each member's plaintext
//! the moment that member's last byte arrives (zstd
//! `IncrementalDecoder`'s per-frame contract).
//!
//! [`streaming_encoder`]: crate::codec::streaming_encoder

#![forbid(unsafe_code)]

use omnizip_codecs::{CodecId, OmnizipError, StreamingDecoder};

use crate::bz2::decompress::{decompress_multi_stream, decompress_one_stream};

/// Streaming bzip2 decoder with per-member incremental emission.
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
        // Emit every member that decodes completely from the buffer.
        // An Err here means "not all buffered bytes form a complete
        // member yet" (truncated tail) OR corruption — both defer to
        // `finish`, which decodes the remainder strictly and reports
        // either condition loudly.
        let mut out = Vec::new();
        while !self.buf.is_empty() {
            match decompress_one_stream(&self.buf) {
                Ok((part, consumed)) => {
                    out.extend_from_slice(&part);
                    self.buf.drain(..consumed);
                }
                Err(_) => break,
            }
        }
        Ok(out)
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
        let mut got = Vec::new();
        let mut i = 0;
        while i < compressed.len() {
            let n = 97.min(compressed.len() - i);
            got.extend_from_slice(&d.write(&compressed[i..i + n]).unwrap());
            i += n;
        }
        got.extend_from_slice(&d.finish().unwrap());
        assert_eq!(got, input);
    }

    #[test]
    fn matches_one_shot_decode() {
        let input: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let compressed = Bzip2Codec
            .compress(&input, CompressionLevel::default())
            .unwrap();
        let mut d = Bzip2StreamingDecoder::new();
        let mut got = Vec::new();
        got.extend_from_slice(&d.write(&compressed[..compressed.len() / 2]).unwrap());
        got.extend_from_slice(&d.write(&compressed[compressed.len() / 2..]).unwrap());
        got.extend_from_slice(&d.finish().unwrap());
        assert_eq!(got, input);
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
        let mut got = Vec::new();
        got.extend_from_slice(&d.write(&compressed).unwrap());
        got.extend_from_slice(&d.finish().unwrap());
        assert!(got.is_empty());
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
