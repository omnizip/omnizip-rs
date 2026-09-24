//! Streaming deflate decoder (omnizip-rs #712).
//!
//! Handles the concatenated zlib-framed form the crate's
//! [`streaming_encoder`] emits (one independent member per chunk),
//! emitting each member's plaintext the moment that member's last
//! byte arrives. Raw DEFLATE has no resync framing, so the walk
//! relies on `inflate_with_consumed` reporting each member's byte
//! count.
//!
//! [`streaming_encoder`]: crate::streaming_encoder

#![forbid(unsafe_code)]

use omnizip_codecs::{CodecId, OmnizipError, StreamingDecoder};

use omnizip_libdeflate::inflate::inflate_with_consumed;

/// Streaming deflate decoder with per-member incremental emission.
pub struct DeflateStreamingDecoder {
    buf: Vec<u8>,
    finished: bool,
}

impl DeflateStreamingDecoder {
    /// Construct a fresh streaming decoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            finished: false,
        }
    }
}

impl Default for DeflateStreamingDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingDecoder for DeflateStreamingDecoder {
    fn write(&mut self, input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
        if self.finished {
            return Err(OmnizipError::DecodeFailed {
                codec: CodecId::DEFLATE,
                reason: "write after finish".into(),
            });
        }
        self.buf.extend_from_slice(input);
        // Emit every member that inflates completely from the buffer.
        // An Err means "incomplete tail" or corruption — both defer to
        // `finish`, which decodes the remainder strictly.
        let mut out = Vec::new();
        loop {
            if self.buf.is_empty() {
                break;
            }
            let (header, trailer) = zlib_framing(&self.buf);
            if header + trailer > self.buf.len() {
                break; // too short to even hold framing — wait for more
            }
            let body = &self.buf[header..self.buf.len() - trailer];
            match inflate_with_consumed(body, body.len().saturating_mul(6)) {
                Ok((part, consumed)) => {
                    out.extend_from_slice(&part);
                    self.buf.drain(..header + consumed + trailer);
                }
                Err(_) => break,
            }
        }
        Ok(out)
    }

    fn finish(self) -> Result<Vec<u8>, OmnizipError> {
        let mut out = Vec::new();
        let mut rest = self.buf.as_slice();
        while !rest.is_empty() {
            // Each member from `DeflateCodec::compress` is zlib-framed
            // (2-byte header + 4-byte adler32 trailer); raw members pass
            // through with no header/trailer adjustment.
            let (header, trailer) = zlib_framing(rest);
            if header + trailer > rest.len() {
                return Err(OmnizipError::DecodeFailed {
                    codec: CodecId::DEFLATE,
                    reason: "member shorter than its zlib framing".into(),
                });
            }
            let body = &rest[header..rest.len() - trailer];
            let (part, consumed) = inflate_with_consumed(body, body.len().saturating_mul(6))
                .map_err(|e| OmnizipError::DecodeFailed {
                    codec: CodecId::DEFLATE,
                    reason: e.to_string(),
                })?;
            if consumed == 0 {
                return Err(OmnizipError::DecodeFailed {
                    codec: CodecId::DEFLATE,
                    reason: "inflate consumed nothing".into(),
                });
            }
            out.extend_from_slice(&part);
            rest = &rest[header + consumed + trailer..];
        }
        Ok(out)
    }
}

/// Detect zlib framing at the member head: returns `(header, trailer)`
/// byte counts. Mirrors `omnizip_libdeflate`'s `strip_zlib_wrapper`
/// detection (CM=8, CINFO≤7, CMF·256+FLG ≡ 0 mod 31).
fn zlib_framing(data: &[u8]) -> (usize, usize) {
    if data.len() >= 6 {
        let cmf = data[0];
        let flg = data[1];
        if cmf & 0x0F == 8
            && (cmf >> 4) & 0x0F <= 7
            && (u16::from(cmf) * 256 + u16::from(flg)) % 31 == 0
        {
            return (2, 4);
        }
    }
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::DeflateStreamingDecoder;
    use crate::streaming_encoder;
    use crate::DeflateCodec;
    use omnizip_codecs::level::CompressionLevel;
    use omnizip_codecs::streaming::{StreamingDecoder, StreamingEncoder};
    use omnizip_codecs::Codec;

    #[test]
    fn round_trips_the_streaming_encoders_concat_output() {
        let input: Vec<u8> = (0..30_000u32).map(|i| (i % 249) as u8).collect();
        let mut e = streaming_encoder(CompressionLevel::default(), 8192);
        e.write(&input).unwrap();
        let compressed = e.finish().unwrap();

        let mut d = DeflateStreamingDecoder::new();
        let mut got = Vec::new();
        let mut i = 0;
        while i < compressed.len() {
            let n = 89.min(compressed.len() - i);
            got.extend_from_slice(&d.write(&compressed[i..i + n]).unwrap());
            i += n;
        }
        got.extend_from_slice(&d.finish().unwrap());
        assert_eq!(got, input);
    }

    #[test]
    fn matches_one_shot_round_trip() {
        let input: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let compressed = DeflateCodec
            .compress(&input, CompressionLevel::default())
            .unwrap();
        let mut d = DeflateStreamingDecoder::new();
        let mut got = Vec::new();
        got.extend_from_slice(&d.write(&compressed[..compressed.len() / 2]).unwrap());
        got.extend_from_slice(&d.write(&compressed[compressed.len() / 2..]).unwrap());
        got.extend_from_slice(&d.finish().unwrap());
        assert_eq!(got, input);
    }

    #[test]
    fn emits_each_member_as_its_last_byte_arrives() {
        let m1 = DeflateCodec
            .compress(b"first member payload", CompressionLevel::default())
            .unwrap();
        let m2 = DeflateCodec
            .compress(b"second member payload", CompressionLevel::default())
            .unwrap();
        let mut d = DeflateStreamingDecoder::new();

        let out = d.write(&m1).unwrap();
        assert_eq!(
            out, b"first member payload",
            "member 1 not emitted on completion"
        );

        let mid = m2.len() / 2;
        assert!(d.write(&m2[..mid]).unwrap().is_empty());
        let out = d.write(&m2[mid..]).unwrap();
        assert_eq!(out, b"second member payload");
        assert!(d.finish().unwrap().is_empty());
    }

    #[test]
    fn truncated_stream_is_an_error_on_finish() {
        let input: Vec<u8> = (0..20_000u32).map(|i| (i % 249) as u8).collect();
        let compressed = DeflateCodec
            .compress(&input, CompressionLevel::default())
            .unwrap();
        let cut = compressed.len() - 2;
        let mut d = DeflateStreamingDecoder::new();
        d.write(&compressed[..cut]).unwrap();
        assert!(d.finish().is_err());
    }
}
