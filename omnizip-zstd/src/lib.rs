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
pub use dict_trainer::{
    train_dictionary, train_dictionary_with, DictTrainer, FastCoverOptions, FastCoverTrainer,
    FrequencyTrainer,
};
pub use frame::{detect_frame_kind, strip_magic, BlockHeader, FrameHeader};
pub use fse::{BitStream, ForwardBitStream, FseDecoder, FseState, Table};
pub use huffman::{HuffmanDecoder, HuffmanTable};
pub use literals::decode_literals_section;
pub use sequences::{decode_sequences_section, Sequence, SequenceExecutor,
                    SequencesSection};

pub use encoder::match_finder::MatchState;

/// Reusable ZSTD compressor that caches the match-finder hash table
/// across calls.
///
/// The free function [`compress`] allocates a fresh `MatchState` on
/// every call. At high levels (≥19) the table is `1 << hash_log` up
/// to 128 MB; allocating and zeroing it dominates wall-time for small
/// inputs.
///
/// `ZstdCompressor` caches the table. On each call:
///
/// 1. If the input size or level changed, [`MatchState::resize_for`]
///    grows or shrinks the table (amortised via `Vec::resize`).
/// 2. [`MatchState::clear`] zeroes the table.
/// 3. The encode pipeline runs without allocating.
///
/// For batch workloads (many small inputs at the same level), this
/// eliminates per-call allocation entirely after the first call.
///
/// ## Example
///
/// ```no_run
/// use omnizip_zstd::{ZstdCompressor, ZstdLevel};
///
/// let mut compressor = ZstdCompressor::new();
/// let inputs: &[&[u8]] = &[b"foo", b"bar", b"baz"];
/// for input in inputs {
///     let compressed = compressor.compress(input, ZstdLevel::Default).unwrap();
///     // ... use compressed
/// }
/// ```
#[derive(Debug)]
pub struct ZstdCompressor {
    match_state: MatchState,
}

impl Default for ZstdCompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl ZstdCompressor {
    /// Construct a new compressor with a default-size match-state
    /// table (hash_log = 7, suitable for small inputs).
    #[must_use]
    pub fn new() -> Self {
        Self {
            match_state: MatchState::new(MatchState::default_hash_log()),
        }
    }

    /// Compress `input` at the given level, reusing the cached
    /// `MatchState`. The output is a complete ZSTD frame, byte-
    /// compatible with what [`compress`] produces.
    ///
    /// # Errors
    ///
    /// Returns [`ZstdError::Corrupt`] on internal encoder failures.
    pub fn compress(
        &mut self,
        input: &[u8],
        level: ZstdLevel,
    ) -> Result<Vec<u8>, ZstdError> {
        let level_u8 = level.as_reference_level();
        let mut params = crate::encoder::cparams::get_params(level_u8);
        params.hash_log =
            crate::encoder::block::cap_hash_log_for_input(params.hash_log, input.len());

        if self.match_state.hash_log() != params.hash_log {
            self.match_state.resize_for(params.hash_log);
        }

        let mut out = Vec::with_capacity(input.len() / 2 + 64);
        crate::encoder::block::encode_frame_into_pub(
            &mut out,
            input,
            &params,
            &mut self.match_state,
        )?;
        Ok(out)
    }

    /// Current hash_log of the cached match-state table. Useful for
    /// diagnostics and for callers that want to pre-warm the
    /// allocation.
    #[must_use]
    pub fn hash_log(&self) -> u32 {
        self.match_state.hash_log()
    }
}

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

    /// ZstdCompressor: reuses MatchState across calls. Output must be
    /// byte-identical to the free-function `compress` (same encoder,
    /// same input ⇒ same output, regardless of state caching).
    #[test]
    fn zstd_compressor_matches_free_function() {
        let input = b"the quick brown fox jumps over the lazy dog ".repeat(50);
        let mut compressor = ZstdCompressor::new();

        for level in [ZstdLevel::Fastest, ZstdLevel::Default, ZstdLevel::Best] {
            let via_free = compress(&input, level).expect("free fn");
            let via_compressor = compressor.compress(&input, level).expect("compressor");
            assert_eq!(
                via_free, via_compressor,
                "ZstdCompressor output must match free function for {:?}",
                level
            );
        }
    }

    /// ZstdCompressor: round-trips through the decoder.
    #[test]
    fn zstd_compressor_round_trips() {
        let mut compressor = ZstdCompressor::new();
        let inputs: Vec<Vec<u8>> = vec![
            b"hello world".to_vec(),
            b"the quick brown fox ".repeat(100),
            (0..10_000).map(|i| (i % 251) as u8).collect(),
        ];
        for input in &inputs {
            let compressed = compressor.compress(input, ZstdLevel::Default).expect("compress");
            let decoded = decompress(&compressed, input.len() as u32).expect("decompress");
            assert_eq!(decoded, *input, "round-trip mismatch");
        }
    }

    /// ZstdCompressor: resize handles varying input sizes. A small
    /// input after a large one must still produce correct output.
    #[test]
    fn zstd_compressor_resizes_for_varying_input_sizes() {
        let mut compressor = ZstdCompressor::new();
        // First compress a large input (forces large hash_log).
        let big: Vec<u8> = (0..50_000).map(|i| (i & 0xFF) as u8).collect();
        let big_out = compressor.compress(&big, ZstdLevel::Default).expect("big");
        // Then a small input (forces small hash_log via cap).
        let small = b"tiny";
        let small_out = compressor.compress(small, ZstdLevel::Default).expect("small");
        // Both must round-trip.
        assert_eq!(decompress(&big_out, big.len() as u32).unwrap(), big);
        assert_eq!(decompress(&small_out, small.len() as u32).unwrap(), small);
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

#[cfg(test)]
mod e2e_fse_tests {
    use super::decompress;

    /// End-to-end test: decompress a real zstd file that contains
    /// FSE-compressed Huffman weights (produced by zstd 1.5.7 CLI).
    /// This exercises the full decode pipeline: frame header, block,
    /// compressed literals, Huffman tree with FSE-compressed weights,
    /// FSE NCount decode, FSE bitstream decode, Huffman decode.
    #[test]
    fn decompresses_zstd_file_with_fse_compressed_huffman_weights() {
        // Build a synthetic input that triggers FSE-compressed weights.
        // The Rust encoder's encode_weights_fse path produces this when
        // the alphabet has > 128 symbols.
        let input: Vec<u8> = (0..100_000)
            .map(|i| {
                if i % 10 < 3 {
                    0u8
                } else if i % 10 < 5 {
                    65 + ((i % 26) as u8)
                } else if i % 10 < 7 {
                    (i % 256) as u8
                } else {
                    ((i * 37) % 256) as u8
                }
            })
            .collect();

        // Compress with our encoder.
        let compressed = super::compress(&input, super::ZstdLevel::Fast)
            .expect("compress");

        // Decompress and verify round-trip.
        let decompressed = decompress(&compressed, input.len() as u32)
            .expect("decompress");

        assert_eq!(decompressed, input,
            "Round-trip failed for FSE-compressed Huffman weights input");
    }

    /// Cross-compatibility test: decode a zstd frame produced by the C
    /// reference library (zstd 1.5.7) that contains FSE-compressed
    /// Huffman weights.
    ///
    /// This is the most important test for the FSE decoder bug report:
    /// it verifies that our Rust decoder can correctly handle frames
    /// produced by the official C implementation.
    #[test]
    fn decodes_c_reference_frame_with_fse_huffman_weights() {
        // This is a minimal zstd frame (31 bytes) that contains ONLY
        // the Huffman tree description with FSE-compressed weights --
        // no literal data, no sequences. It was extracted from a real
        // zstd -3 compressed file produced by zstd CLI v1.5.7.
        //
        // The FSE payload (30 bytes after the tree byte 0x1e) encodes
        // a 256-symbol Huffman weight table at tableLog=5.
        let fse_header: &[u8] = &[
            0x1e, 0x10, 0xd8, 0xda, 0x72, 0x0c, 0x03, 0xb8,
            0xa2, 0x61, 0x70, 0x4d, 0x92, 0x3a, 0x91, 0x6e,
            0xa1, 0x26, 0x12, 0xd9, 0x6e, 0xa1, 0xa5, 0x95,
            0xed, 0x16, 0x35, 0x0c, 0x53, 0x91, 0x02,
        ];

        let (table, consumed) = crate::huffman::weights::read_huffman_table(fse_header)
            .expect("read FSE-compressed Huffman table");

        // The table should have 256 symbols (255 FSE-decoded + 1 implied).
        assert_eq!(table.symbol_count(), 256,
            "Huffman table should have 256 symbols");
        assert_eq!(consumed, 31,
            "Should consume all 31 bytes (tree byte + 30 FSE bytes)");

        // Verify the weights match the C reference output.
        let weights = table.weights();
        assert_eq!(weights[0], 8, "symbol 0 weight");
        assert_eq!(weights[142], 2, "symbol 142 weight");
        assert_eq!(weights[195], 2, "symbol 195 weight");
        assert_eq!(weights[212], 2, "symbol 212 weight");
        assert_eq!(weights[255], 1, "implied last weight");
    }
}
