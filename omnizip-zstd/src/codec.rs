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
