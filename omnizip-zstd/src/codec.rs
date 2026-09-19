//! `ZstdCodec` — adapts the ZSTD encoder + decoder to the
//! `omnizip_codecs::Codec` trait.

#![forbid(unsafe_code)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

use crate::{decompress, ZstdDecoder, ZstdError};

/// Codec entry for the Zstandard format.
pub struct ZstdCodec;

impl ZstdCodec {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for ZstdCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl Codec for ZstdCodec {
    fn id(&self) -> CodecId {
        CodecId::ZSTD
    }

    fn name(&self) -> &'static str {
        "zstd"
    }

    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        // Map the omnizip CompressionLevel (0-22) directly to the ZSTD
        // reference level (1-22), using the full cparams table from
        // `clevels.h`. This gives fine-grained level differentiation:
        // each level has its own (window_log, chain_log, hash_log,
        // search_log, min_match, target_length, strategy) tuple.
        //
        // Previously this collapsed 22 levels into just 5 ZstdLevel
        // enum values, losing the per-level parameter tuning.
        let zstd_level = level.as_u8().clamp(1, 22);
        crate::encoder::block::encode_frame_compressed(plaintext, zstd_level).map_err(|e| {
            OmnizipError::EncodeFailed {
                codec: CodecId::ZSTD,
                reason: e.to_string(),
            }
        })
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let _ = ZstdDecoder::new(); // ensure constructor is referenced
        let out = decompress(compressed, expected_len).map_err(|e| match e {
            ZstdError::Unsupported { reason } => OmnizipError::Unsupported {
                codec: CodecId::ZSTD,
                reason,
            },
            other => OmnizipError::DecodeFailed {
                codec: CodecId::ZSTD,
                reason: other.to_string(),
            },
        })?;
        let expected = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::ZSTD,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;
        if out.len() != expected {
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::ZSTD,
                expected: expected_len,
                actual: out.len(),
            });
        }
        Ok(out)
    }

    fn default_fast_level(&self) -> u8 {
        1
    }
    fn default_balanced_level(&self) -> u8 {
        9
    }
    fn default_max_ratio_level(&self) -> u8 {
        19
    }

    fn capabilities(&self) -> omnizip_codecs::Capabilities {
        omnizip_codecs::Capabilities {
            min_level: 1,
            max_level: 22,
            streaming: false, // TODO 251: streaming impl pending
            parallel_batch: true,
            has_static_dictionary: false, // dictionaries are user-supplied
            content_type_aware: true,
            approx_throughput_mbps: 100,
        }
    }
}

/// ZSTD memory budget: input + output + window + hash tables.
/// Window scales with `window_log` (10..23 depending on level).
impl omnizip_codecs::MemoryBudget for ZstdCodec {
    fn estimated_compress_memory(
        &self,
        input_len: usize,
        level: omnizip_codecs::CompressionLevel,
    ) -> usize {
        let lv = level.as_u8().min(22);
        // window_log scales 10..23; hash_log similarly.
        let window_log: u32 = if lv <= 5 {
            10
        } else if lv <= 12 {
            18
        } else if lv <= 19 {
            21
        } else {
            23
        };
        let window = 1usize << window_log;
        let hash_table = (1usize << window_log) * 4;
        input_len + input_len / 2 + window + hash_table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_id_is_zstd() {
        assert_eq!(ZstdCodec::new().id(), CodecId::ZSTD);
    }

    #[test]
    fn round_trip_via_codec() {
        let codec = ZstdCodec::new();
        let input = b"hello zstd codec world";
        let compressed = codec
            .compress(input, CompressionLevel::default())
            .expect("encode");
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .expect("decode");
        assert_eq!(decompressed, input);
    }

    /// omnizip issue #315 residual (BUGREPORT-zstd-315-residual.md): the
    /// 163-byte mixed text+binary input whose frame (identical 172 B at
    /// levels 1/3/5/9) our own decoder mis-reconstructed at 0.16.78.
    /// Fixed by the 0.16.87-0.16.96 sequence/literal section rewrites;
    /// pinned so the few-sequences/small-block edge can't regress.
    #[test]
    fn issue_315_blob_round_trips_all_levels() {
        const B64: &str = "LwjOGAEAAAAEpQAAAGR1cGxpY2F0ZSBpbmxpbmUgY29udGVuaGUgc2FtZSAyMDAtaXNoIGJ5dGVzIGluIHRocmVlIGZpbGVzLCBzbyB0aGUgd3JpdGVyJ2VzIG9uIGV2ZXJ5IHJlYWxpc3RpYyB0cmVlLiBQYWQAAAAF6UCBLwjOAQAAAADSf+9PzA2Fv8RqcmiN5Gtx/fn2pu5LCCNiKcneAQ==";
        fn b64(s: &str) -> Vec<u8> {
            let s: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
            let (mut out, mut acc, mut nbits) = (Vec::with_capacity(s.len() / 4 * 3), 0u32, 0);
            for b in s {
                if b == b'=' {
                    break;
                }
                let v = match b {
                    b'A'..=b'Z' => b - b'A',
                    b'a'..=b'z' => b - b'a' + 26,
                    b'0'..=b'9' => b - b'0' + 52,
                    b'+' => 62,
                    b'/' => 63,
                    _ => panic!("bad b64"),
                } as u32;
                acc = (acc << 6) | v;
                nbits += 6;
                if nbits >= 8 {
                    nbits -= 8;
                    out.push(((acc >> nbits) & 0xFF) as u8);
                }
            }
            out
        }
        let raw = b64(B64);
        assert_eq!(raw.len(), 163);
        let codec = ZstdCodec::new();
        for lv in [1u8, 3, 5, 9, 19] {
            let c = codec.compress(&raw, CompressionLevel::new(lv)).unwrap();
            let out = codec.decompress(&c, raw.len() as u32).unwrap();
            assert_eq!(out, raw, "round trip failed at level {}", lv);
        }
    }
}

/// Streaming zstd decoder (TODO.ref-parity/57, decoder leg): v1
/// buffers the compressed stream and decodes at `finish`
/// (`expected_len = u32::MAX` uses the length-agnostic path).
/// Output equals the one-shot [`decompress`] exactly.
#[must_use]
pub fn streaming_decoder(expected_len: u32) -> ZstdStreamingDecoder {
    ZstdStreamingDecoder {
        incremental: expected_len == u32::MAX,
        inner: None,
        legacy: None,
    }
}

/// Streaming zstd decode: unknown-length (`u32::MAX`) callers get
/// TRUE incremental decoding (per-frame span parsing — a complete
/// frame's plaintext is returned by the very `write` that completed
/// it); exact-length callers get the buffered decode-at-finish
/// leg. Both produce the one-shot output exactly.
pub struct ZstdStreamingDecoder {
    incremental: bool,
    inner: Option<crate::incremental::IncrementalDecoder>,
    legacy: Option<omnizip_codecs::streaming::ChunkedStreamDecoder>,
}

impl omnizip_codecs::streaming::StreamingDecoder for ZstdStreamingDecoder {
    fn write(&mut self, input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
        if self.incremental {
            let inner = self
                .inner
                .get_or_insert_with(crate::incremental::IncrementalDecoder::new);
            return inner.write(input).map_err(|e| OmnizipError::DecodeFailed {
                codec: CodecId::ZSTD,
                reason: e.to_string(),
            });
        }
        let legacy = self.legacy.get_or_insert_with(|| {
            omnizip_codecs::streaming::ChunkedStreamDecoder::new(
                Box::new(LenientZstdCodec),
                u32::MAX,
            )
        });
        legacy.write(input)
    }

    fn finish(mut self) -> Result<Vec<u8>, OmnizipError> {
        if self.incremental {
            let mut inner = self
                .inner
                .take()
                .unwrap_or_else(crate::incremental::IncrementalDecoder::new);
            return inner.finish().map_err(|e| OmnizipError::DecodeFailed {
                codec: CodecId::ZSTD,
                reason: e.to_string(),
            });
        }
        self.legacy
            .take()
            .map_or_else(|| Ok(Vec::new()), |legacy| legacy.finish())
    }
}

/// ZstdCodec except `decompress` ignores `expected_len` — wraps the
/// length-agnostic free function. Used by [`streaming_decoder`]
/// for unknown-length (streaming) callers; the strict codec stays
/// the registry default.
struct LenientZstdCodec;

impl Codec for LenientZstdCodec {
    fn id(&self) -> CodecId {
        CodecId::ZSTD
    }
    fn name(&self) -> &'static str {
        "zstd"
    }
    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        ZstdCodec.compress(plaintext, level)
    }
    fn decompress(&self, compressed: &[u8], _expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        crate::decompress(compressed, u32::MAX).map_err(|e| OmnizipError::DecodeFailed {
            codec: CodecId::ZSTD,
            reason: e.to_string(),
        })
    }
}

/// Bounded-memory streaming zstd encoder (TODO.ref-parity/57): one
/// independent frame per `chunk_size` plaintext bytes, concatenated
/// (multi-frame output — decodes with [`decompress`] and any zstd
/// CLI). Output is a pure function of (input, chunk_size): push
/// partitioning never affects the bytes.
#[must_use]
pub fn streaming_encoder(
    level: CompressionLevel,
    chunk_size: usize,
) -> omnizip_codecs::streaming::ChunkedStreamEncoder {
    omnizip_codecs::streaming::ChunkedStreamEncoder::new(Box::new(ZstdCodec), level, chunk_size)
}

#[cfg(test)]
mod streaming_tests {
    use super::streaming_encoder;
    use crate::decompress;
    use omnizip_codecs::level::CompressionLevel;
    use omnizip_codecs::streaming::StreamingEncoder;

    fn data(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| u8::try_from(i % 251).expect("<251"))
            .collect()
    }

    #[test]
    fn multi_frame_round_trips_through_our_decoder() {
        for (len, chunk) in [(0_usize, 64), (1, 64), (5000, 1024)] {
            let input = data(len);
            let mut e = streaming_encoder(CompressionLevel::default(), chunk);
            e.write(&input).unwrap();
            let out = e.finish().unwrap();
            let decoded = decompress(&out, u32::MAX).unwrap();
            assert_eq!(decoded, input, "len {len} chunk {chunk}");
        }
    }

    #[test]
    fn partition_invariance_on_real_zstd_bytes() {
        let input = data(8192);
        let level = CompressionLevel::default();
        let baseline = {
            let mut e = streaming_encoder(level, 1024);
            e.write(&input).unwrap();
            e.finish().unwrap()
        };
        // Two adversarial partitions: single-byte writes and one
        // giant write.
        let dribble = {
            let mut e = streaming_encoder(level, 1024);
            for b in &input {
                e.write(std::slice::from_ref(b)).unwrap();
            }
            e.finish().unwrap()
        };
        let one_shot_write = {
            let mut e = streaming_encoder(level, 1024);
            e.write(&input).unwrap();
            e.finish().unwrap()
        };
        assert_eq!(baseline, dribble);
        assert_eq!(baseline, one_shot_write);
    }
}

#[cfg(test)]
mod streaming_decoder_tests {
    use crate::codec::streaming_decoder;
    use omnizip_codecs::streaming::StreamingDecoder;

    #[test]
    fn streaming_decode_matches_one_shot() {
        let input: Vec<u8> = (0..30_000u32).map(|i| (i % 251) as u8).collect();
        let compressed = crate::encoder::block::encode_frame_compressed(&input, 6).unwrap();
        let mut d = streaming_decoder(u32::MAX);
        // Incremental contract: output arrives AT WRITE for every
        // completed frame; finish returns only the tail. Adversarial
        // 13-byte partition; a single frame completes only at the end.
        let mut got = Vec::new();
        let mut i = 0;
        while i < compressed.len() {
            let n = 13.min(compressed.len() - i);
            got.extend_from_slice(&d.write(&compressed[i..i + n]).unwrap());
            i += n;
        }
        got.extend_from_slice(&d.finish().unwrap());
        assert_eq!(got, input);
    }

    #[test]
    fn streaming_decode_is_incremental_across_frames() {
        // Multi-frame stream: a complete frame must decode DURING
        // write, not only at finish.
        let a: Vec<u8> = std::iter::repeat(b'x').take(5000).collect();
        let b: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let fa = crate::encoder::block::encode_frame_compressed(&a, 3).unwrap();
        let fb = crate::encoder::block::encode_frame_compressed(&b, 3).unwrap();

        let mut d = streaming_decoder(u32::MAX);
        let out1 = d.write(&fa).unwrap();
        assert_eq!(out1, a, "first frame did not decode incrementally");
        let out2 = d.write(&fb).unwrap();
        assert_eq!(out2, b, "second frame did not decode incrementally");
        assert!(d.finish().unwrap().is_empty());
    }
}
