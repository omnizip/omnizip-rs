//! omnizip-zstd — Pure-Rust Zstandard.
//!
//! Rust port of omnizip's Ruby ZSTD reference at
//! `omnizip/lib/omnizip/algorithms/zstandard/` (3,150 LOC).
//!
//! See the workspace [`PLAN.md`](../../PLAN.md) for the Ruby → Rust module
//! map and the phased delivery plan.
//!
//! ## Status
//!
//! **Phase A: foundation + RAW/RLE block decode working.** Constants,
//! frame header parser, FSE bitstream + table, block header, and the
//! top-level [`decoder::ZstdDecoder`] are ported. End-to-end decode
//! works for streams containing Raw / RLE blocks (small inputs that
//! `zstd` chooses not to compress). Compressed blocks need the
//! Huffman + literals + sequences stack, which lands next.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
// Codec code performs extensive byte↔usize↔u32 conversions where the
// value ranges are guaranteed by upstream protocol checks. The pedantic
// cast lints fire on every such conversion without knowing the invariant;
// they're more useful at API boundaries than on every arithmetic site.
#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_lossless)]

pub mod codec;
pub mod constants;
pub mod decoder;
pub mod dict;
pub mod dict_trainer;
pub mod encoder;
pub mod frame;
pub mod fse;
pub mod huffman;
pub mod literals;
pub mod predef_tables;
pub mod sequences;
pub mod xxhash;

use std::fmt;

pub use codec::ZstdCodec;
pub use constants::{
    BLOCK_HEADER_SIZE, BLOCK_MAX_SIZE, BLOCK_TYPE_COMPRESSED, BLOCK_TYPE_RAW, BLOCK_TYPE_RLE,
    DEFAULT_LEVEL, FSE_MAX_ACCURACY_LOG, FSE_MIN_ACCURACY_LOG, MAGIC_BYTES, MAGIC_NUMBER,
    MAX_LEVEL, MIN_LEVEL, WINDOW_LOG_MAX, WINDOW_LOG_MIN,
};
pub use decoder::ZstdDecoder;
pub use dict::ZstdDictionary;
pub use dict_trainer::train_dictionary;
pub use frame::{detect_frame_kind, strip_magic, BlockHeader, FrameHeader};
pub use fse::{BitStream, ForwardBitStream, FseDecoder, FseState, Table};
pub use huffman::{HuffmanDecoder, HuffmanTable};
pub use literals::decode_literals_section;
pub use sequences::{decode_sequences_section, Sequence, SequenceExecutor,
                    SequencesSection};
/// ZSTD compression level. Mirrors the reference `zstd` encoder scale.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ZstdLevel {
    /// `zstd -1`.
    Fastest,
    /// `zstd -3`.
    Fast,
    /// `zstd -6` (the `zstd` default).
    Default,
    /// `zstd -12`.
    Better,
    /// `zstd -22` (best ratio, slowest encode).
    Best,
}

impl ZstdLevel {
    /// Numeric level matching the reference `zstd` encoder.
    #[must_use]
    pub fn as_reference_level(self) -> u8 {
        match self {
            Self::Fastest => 1,
            Self::Fast => 3,
            Self::Default => 6,
            Self::Better => 12,
            Self::Best => 22,
        }
    }
}

impl fmt::Display for ZstdLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "zstd-{}", self.as_reference_level())
    }
}

/// Error type. Will grow as phases ship.
#[derive(Debug)]
pub enum ZstdError {
    /// Level not supported by this codec.
    LevelUnavailable(ZstdLevel),
    /// Malformed input.
    Corrupt { reason: String },
    /// Feature not available in this codec.
    /// Huffman + literals + sequences stack lands).
    Unsupported { reason: String },
}

impl fmt::Display for ZstdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LevelUnavailable(level) => write!(f, "level {level} not supported by this codec"),
            Self::Corrupt { reason } => write!(f, "corrupt zstd frame: {reason}"),
            Self::Unsupported { reason } => write!(f, "unsupported: {reason}"),
        }
    }
}

impl std::error::Error for ZstdError {}

/// Compress `plaintext` at the given level.
///
/// Compress `plaintext` into a ZSTD frame at the given level.
/// slightly larger than the input but round-trips through any ZSTD
/// decoder including the reference C implementation.
///
/// # Errors
///
/// See [`encoder::encode_frame`].
pub fn compress(plaintext: &[u8], level: ZstdLevel) -> Result<Vec<u8>, ZstdError> {
    encoder::encode_frame(plaintext, level)
}

/// Decompress a ZSTD frame.
///
/// Currently decodes Raw, RLE, and Compressed blocks (Compressed
/// requires the literals + sequences + executor stack). The
/// FSE-compressed Huffman weights path is available via the decoder.
/// BUGREPORT.01).
///
/// # Errors
///
/// See [`ZstdDecoder::decode_stream`].
pub fn decompress(compressed: &[u8], _expected_len: u32) -> Result<Vec<u8>, ZstdError> {
    let mut decoder = ZstdDecoder::new();
    decoder.decode_stream(compressed)
}

/// Compress `plaintext` at the given level using a dictionary.
///
/// The dictionary content is presented to the match finder as a
/// prefix to `plaintext`, so the encoder can emit back-references
/// into dictionary bytes. The resulting frame is a valid standalone
/// ZSTD frame whose `Frame_Content_Size` reflects `dict_content.len()
/// + plaintext.len()`; the dict-aware decompress path
/// ([`decompress_with_dict`]) strips the dictionary prefix.
///
/// # Errors
///
/// See [`encoder::encode_frame_with_dict`].
pub fn compress_with_dict(
    plaintext: &[u8],
    level: ZstdLevel,
    dict: &ZstdDictionary,
) -> Result<Vec<u8>, ZstdError> {
    encoder::encode_frame_with_dict(plaintext, level.as_reference_level(), dict)
}

/// Decompress a ZSTD frame produced by [`compress_with_dict`].
///
/// Primes the decoder's output window with the dictionary content so
/// back-references into the dictionary resolve correctly, then
/// decodes the frame. The returned bytes are the original plaintext
/// (the dictionary prefix is stripped internally).
///
/// `expected_len` is the expected length of the *plaintext* (not
/// including the dictionary prefix). It is currently unused but kept
/// for API symmetry with [`decompress`].
///
/// # Errors
///
/// See [`ZstdDecoder::decode_stream_with_prefix`].
pub fn decompress_with_dict(
    compressed: &[u8],
    _expected_len: u32,
    dict: &ZstdDictionary,
) -> Result<Vec<u8>, ZstdError> {
    let mut decoder = ZstdDecoder::new();
    decoder.decode_stream_with_prefix(compressed, dict.content())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_displays_reference_value() {
        assert_eq!(ZstdLevel::Fastest.to_string(), "zstd-1");
        assert_eq!(ZstdLevel::Default.to_string(), "zstd-6");
        assert_eq!(ZstdLevel::Best.to_string(), "zstd-22");
    }

    #[test]
    fn compress_round_trips() {
        let input = b"abc";
        let compressed = compress(input, ZstdLevel::Default).expect("encode");
        let decompressed = decompress(&compressed, input.len() as u32).expect("decode");
        assert_eq!(decompressed, input);
    }

    // ---- Dictionary integration tests ----

    fn json_corpus() -> Vec<Vec<u8>> {
        // Small JSON samples sharing structure + key names — exactly
        // the workload dictionaries are designed for.
        (0..20)
            .map(|i| {
                format!(
                    "{{\"id\":{i},\"name\":\"item{i}\",\"type\":\"product\",\"price\":{},{},\"tags\":[\"a\",\"b\",\"c\"]}}",
                    10 + i,
                    "\"stock\":"
                )
                .into_bytes()
            })
            .collect()
    }

    #[test]
    fn dict_compress_round_trips() {
        let corpus = json_corpus();
        let refs: Vec<&[u8]> = corpus.iter().map(Vec::as_slice).collect();
        let content = train_dictionary(&refs, 4096);
        let dict = ZstdDictionary::from_raw(42, &content);

        // Compress a new sample (not in the corpus) with the dict.
        let sample = b"{\"id\":99,\"name\":\"newitem\",\"type\":\"product\",\"price\":50}".to_vec();
        let compressed =
            compress_with_dict(&sample, ZstdLevel::Default, &dict).expect("encode with dict");
        let decompressed =
            decompress_with_dict(&compressed, sample.len() as u32, &dict).expect("decode with dict");
        assert_eq!(decompressed, sample);
    }

    #[test]
    fn dict_compress_smaller_than_without_dict() {
        // Use highly repetitive samples so the dictionary captures
        // real redundancy the no-dict path can't exploit on a fresh
        // small input.
        let corpus: Vec<Vec<u8>> = (0..30)
            .map(|i| {
                format!(
                    "function handler_{i}() {{ return CONSTANT_PREFIX_{i} + SHARED_SUFFIX; }}\n"
                )
                .into_bytes()
            })
            .collect();
        let refs: Vec<&[u8]> = corpus.iter().map(Vec::as_slice).collect();
        let content = train_dictionary(&refs, 8192);
        let dict = ZstdDictionary::from_raw(7, &content);

        // New sample that shares the corpus's common substrings.
        let sample = b"function handler_X() { return CONSTANT_PREFIX_X + SHARED_SUFFIX; }\n".to_vec();

        let with_dict =
            compress_with_dict(&sample, ZstdLevel::Default, &dict).expect("encode with dict");
        let without_dict = compress(&sample, ZstdLevel::Default).expect("encode no dict");

        assert!(
            with_dict.len() < without_dict.len(),
            "dict-compressed ({}) should be smaller than no-dict ({})",
            with_dict.len(),
            without_dict.len()
        );
    }

    #[test]
    fn dict_serialization_round_trips() {
        let dict = ZstdDictionary::from_raw(0x1234, b"some dictionary content");
        let blob = dict.serialize();
        let dict2 = ZstdDictionary::deserialize(&blob).expect("deserialize");
        assert_eq!(dict, dict2);
    }

    #[test]
    fn frame_header_includes_dictionary_id() {
        let dict = ZstdDictionary::from_raw(0xAB, b"content");
        let sample = b"some input bytes to compress";
        let compressed = compress_with_dict(sample, ZstdLevel::Default, &dict).expect("encode");

        // Magic (4 bytes) then descriptor. The Dictionary_ID_flag
        // lives in bits 0-1 of the descriptor.
        let descriptor = compressed[4];
        let did_flag = descriptor & 0x03;
        assert!(did_flag != 0, "expected Dictionary_ID_flag != 0, got {did_flag}");

        // For id = 0xAB (≤ 255), flag should be 1 (1-byte field).
        assert_eq!(did_flag, 1, "expected 1-byte Dictionary_ID encoding");
        // The Dictionary_ID follows the descriptor (and before FCS,
        // since single_segment=1 means no window_descriptor).
        // Layout: descriptor, Dictionary_ID (1 byte), FCS (1+ bytes).
        assert_eq!(compressed[5], 0xAB);
    }

    #[test]
    fn dict_with_empty_content_round_trips() {
        let dict = ZstdDictionary::from_raw(1, b"");
        let sample = b"hello world";
        let compressed = compress_with_dict(sample, ZstdLevel::Default, &dict).expect("encode");
        let decompressed = decompress_with_dict(&compressed, sample.len() as u32, &dict).expect("decode");
        assert_eq!(decompressed, sample);
    }

    #[test]
    fn dict_round_trip_multiple_levels() {
        let corpus = json_corpus();
        let refs: Vec<&[u8]> = corpus.iter().map(Vec::as_slice).collect();
        let content = train_dictionary(&refs, 2048);
        let dict = ZstdDictionary::from_raw(99, &content);

        let sample = b"{\"id\":500,\"name\":\"test\",\"type\":\"product\"}".to_vec();
        for level in [ZstdLevel::Fastest, ZstdLevel::Default, ZstdLevel::Better] {
            let compressed = compress_with_dict(&sample, level, &dict)
                .unwrap_or_else(|e| panic!("encode {level} failed: {e:?}"));
            let decompressed = decompress_with_dict(&compressed, sample.len() as u32, &dict)
                .unwrap_or_else(|e| panic!("decode {level} failed: {e:?}"));
            assert_eq!(decompressed, sample, "round-trip failed at {level}");
        }
    }

    #[test]
    fn dict_compress_is_deterministic() {
        let corpus = json_corpus();
        let refs: Vec<&[u8]> = corpus.iter().map(Vec::as_slice).collect();
        let content = train_dictionary(&refs, 1024);
        let dict = ZstdDictionary::from_raw(5, &content);

        let sample = b"{\"id\":1,\"name\":\"x\",\"type\":\"y\"}".to_vec();
        let a = compress_with_dict(&sample, ZstdLevel::Default, &dict).expect("encode a");
        let b = compress_with_dict(&sample, ZstdLevel::Default, &dict).expect("encode b");
        assert_eq!(a, b, "dict-compress output non-deterministic");
    }
}
