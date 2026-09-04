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
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

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
pub use sequences::{
    decode_sequences_section, SeqTableState, Sequence, SequenceExecutor, SequencesSection,
};

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
    /// table (`hash_log` = 7, suitable for small inputs).
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
    pub fn compress(&mut self, input: &[u8], level: ZstdLevel) -> Result<Vec<u8>, ZstdError> {
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

    /// Current `hash_log` of the cached match-state table. Useful for
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

/// Compress `plaintext` at the given level across multiple threads.
///
/// Opt-in multi-threaded variant of [`compress`]: the input is split
/// into fixed-size jobs (a pure function of input length — never of
/// `threads`), each encoded as an independent frame on a scoped
/// worker thread, concatenated in job order. Output is deterministic
/// across thread counts; `threads <= 1` falls back to [`compress`].
///
/// Cross-job matches are lost at job boundaries, so multi-job output
/// can be slightly larger than [`compress`] on highly redundant
/// inputs — measure the delta for your workload
/// (`TODO.remaining/19` records the corpus numbers).
///
/// # Errors
///
/// See [`compress`].
pub fn compress_mt(
    plaintext: &[u8],
    level: ZstdLevel,
    threads: usize,
) -> Result<Vec<u8>, ZstdError> {
    encoder::encode_frames_mt(plaintext, level, threads)
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
    fn compress_mt_round_trips_and_is_thread_invariant() {
        // 5 jobs at the forced 64 KiB job size: exercises the
        // multi-frame path and proves output does not depend on the
        // thread count.
        std::env::set_var("ZSTD_MT_JOB", "65536");
        let mut data = Vec::with_capacity(300 * 1024);
        for i in 0..300 * 1024 {
            data.push(((i / 97) % 251) as u8 ^ (i % 13) as u8);
            if i % 4096 == 0 {
                data.extend_from_slice(b"repeated anchor payload line\n");
            }
        }
        data.truncate(300 * 1024);
        for level in [ZstdLevel::Fastest, ZstdLevel::Default, ZstdLevel::Best] {
            let two = compress_mt(&data, level, 2).expect("2 threads");
            let four = compress_mt(&data, level, 4).expect("4 threads");
            let eight = compress_mt(&data, level, 8).expect("8 threads");
            assert_eq!(two, four, "thread count changed the output");
            assert_eq!(two, eight, "thread count changed the output");
            let plain = decompress(&two, data.len() as u32).expect("decode");
            assert_eq!(plain, data);
        }
        std::env::remove_var("ZSTD_MT_JOB");
    }

    #[test]
    fn compress_mt_small_input_matches_compress() {
        let data = b"single-job inputs must fall through to compress()".to_vec();
        let a = compress_mt(&data, ZstdLevel::Default, 8).expect("mt");
        let b = compress(&data, ZstdLevel::Default).expect("st");
        assert_eq!(a, b);
    }

    /// Regression (user report, 100 MB benchmark): the long-distance
    /// matcher emitted offsets our decoder resolved to different
    /// bytes on inputs past the first LDM table refill — Best failed
    /// its frame checksum above ~1 MiB. LDM is disabled by default;
    /// this pins the round-trip through the previously failing band.
    #[test]
    fn best_round_trips_past_first_ldm_refill() {
        let mut data = Vec::new();
        for i in 0..60_000u32 {
            data.extend_from_slice(format!("row {i}, payload text with structure\n").as_bytes());
        }
        data.truncate(1_500_000);
        let c = crate::compress(&data, ZstdLevel::Best).expect("encode");
        let out = crate::decompress(&c, data.len() as u32).expect("decode");
        assert_eq!(out, data);
    }

    #[test]
    fn decodes_reference_cli_frames() {
        // Regression (BUGREPORT-zstd-0.1.0): the offset-code table
        // (OF_BASE/OF_BITS) didn't match the C reference, so any frame
        // produced by the real `zstd` CLI decoded to garbage. These blobs
        // are `zstd -1` output of a 1000-byte CSV prefix — with and
        // without content checksum — which exercises FSE-compressed
        // sequences and Huffman-compressed literals.
        let input: Vec<u8> = vec![
            34, 67, 65, 32, 79, 119, 110, 101, 114, 34, 44, 34, 83, 97, 108, 101, 115, 102, 111,
            114, 99, 101, 32, 82, 101, 99, 111, 114, 100, 32, 73, 68, 34, 44, 34, 67, 101, 114,
            116, 105, 102, 105, 99, 97, 116, 101, 32, 78, 97, 109, 101, 34, 44, 34, 80, 97, 114,
            101, 110, 116, 32, 83, 97, 108, 101, 115, 102, 111, 114, 99, 101, 32, 82, 101, 99, 111,
            114, 100, 32, 73, 68, 34, 44, 34, 80, 97, 114, 101, 110, 116, 32, 67, 101, 114, 116,
            105, 102, 105, 99, 97, 116, 101, 32, 78, 97, 109, 101, 34, 44, 34, 67, 101, 114, 116,
            105, 102, 105, 99, 97, 116, 101, 32, 82, 101, 99, 111, 114, 100, 32, 84, 121, 112, 101,
            34, 44, 34, 82, 101, 118, 111, 99, 97, 116, 105, 111, 110, 32, 83, 116, 97, 116, 117,
            115, 34, 44, 34, 83, 72, 65, 45, 50, 53, 54, 32, 70, 105, 110, 103, 101, 114, 112, 114,
            105, 110, 116, 34, 44, 34, 80, 97, 114, 101, 110, 116, 32, 83, 72, 65, 45, 50, 53, 54,
            32, 70, 105, 110, 103, 101, 114, 112, 114, 105, 110, 116, 34, 44, 34, 65, 117, 100,
            105, 116, 115, 32, 83, 97, 109, 101, 32, 97, 115, 32, 80, 97, 114, 101, 110, 116, 63,
            34, 44, 34, 65, 117, 100, 105, 116, 111, 114, 34, 44, 34, 83, 116, 97, 110, 100, 97,
            114, 100, 32, 65, 117, 100, 105, 116, 32, 85, 82, 76, 34, 44, 34, 83, 116, 97, 110,
            100, 97, 114, 100, 32, 65, 117, 100, 105, 116, 32, 84, 121, 112, 101, 34, 44, 34, 83,
            116, 97, 110, 100, 97, 114, 100, 32, 65, 117, 100, 105, 116, 32, 83, 116, 97, 116, 101,
            109, 101, 110, 116, 32, 68, 97, 116, 101, 34, 44, 34, 83, 116, 97, 110, 100, 97, 114,
            100, 32, 65, 117, 100, 105, 116, 32, 80, 101, 114, 105, 111, 100, 32, 83, 116, 97, 114,
            116, 32, 68, 97, 116, 101, 34, 44, 34, 83, 116, 97, 110, 100, 97, 114, 100, 32, 65,
            117, 100, 105, 116, 32, 80, 101, 114, 105, 111, 100, 32, 69, 110, 100, 32, 68, 97, 116,
            101, 34, 44, 34, 66, 82, 32, 65, 117, 100, 105, 116, 32, 85, 82, 76, 34, 44, 34, 66,
            82, 32, 65, 117, 100, 105, 116, 32, 84, 121, 112, 101, 34, 44, 34, 66, 82, 32, 65, 117,
            100, 105, 116, 32, 83, 116, 97, 116, 101, 109, 101, 110, 116, 32, 68, 97, 116, 101, 34,
            44, 34, 66, 82, 32, 65, 117, 100, 105, 116, 32, 80, 101, 114, 105, 111, 100, 32, 83,
            116, 97, 114, 116, 32, 68, 97, 116, 101, 34, 44, 34, 66, 82, 32, 65, 117, 100, 105,
            116, 32, 80, 101, 114, 105, 111, 100, 32, 69, 110, 100, 32, 68, 97, 116, 101, 34, 44,
            34, 69, 86, 32, 83, 83, 76, 32, 65, 117, 100, 105, 116, 32, 85, 82, 76, 34, 44, 34, 69,
            86, 32, 83, 83, 76, 32, 65, 117, 100, 105, 116, 32, 84, 121, 112, 101, 34, 44, 34, 69,
            86, 32, 83, 83, 76, 32, 65, 117, 100, 105, 116, 32, 83, 116, 97, 116, 101, 109, 101,
            110, 116, 32, 68, 97, 116, 101, 34, 44, 34, 69, 86, 32, 83, 83, 76, 32, 65, 117, 100,
            105, 116, 32, 80, 101, 114, 105, 111, 100, 32, 83, 116, 97, 114, 116, 32, 68, 97, 116,
            101, 34, 44, 34, 69, 86, 32, 83, 83, 76, 32, 65, 117, 100, 105, 116, 32, 80, 101, 114,
            105, 111, 100, 32, 69, 110, 100, 32, 68, 97, 116, 101, 34, 44, 34, 69, 86, 32, 67, 111,
            100, 101, 32, 83, 105, 103, 110, 105, 110, 103, 32, 65, 117, 100, 105, 116, 32, 85, 82,
            76, 34, 44, 34, 69, 86, 32, 67, 111, 100, 101, 32, 83, 105, 103, 110, 105, 110, 103,
            32, 65, 117, 100, 105, 116, 32, 84, 121, 112, 101, 34, 44, 34, 69, 86, 32, 67, 111,
            100, 101, 32, 83, 105, 103, 110, 105, 110, 103, 32, 65, 117, 100, 105, 116, 32, 83,
            116, 97, 116, 101, 109, 101, 110, 116, 32, 68, 97, 116, 101, 34, 44, 34, 69, 86, 32,
            67, 111, 100, 101, 32, 83, 105, 103, 110, 105, 110, 103, 32, 65, 117, 100, 105, 116,
            32, 80, 101, 114, 105, 111, 100, 32, 83, 116, 97, 114, 116, 32, 68, 97, 116, 101, 34,
            44, 34, 69, 86, 32, 67, 111, 100, 101, 32, 83, 105, 103, 110, 105, 110, 103, 32, 65,
            117, 100, 105, 116, 32, 80, 101, 114, 105, 111, 100, 32, 69, 110, 100, 32, 68, 97, 116,
            101, 34, 44, 34, 67, 80, 47, 67, 80, 83, 32, 83, 97, 109, 101, 32, 97, 115, 32, 80, 97,
            114, 101, 110, 116, 63, 34, 44, 34, 67, 101, 114, 116, 105, 102, 105, 99, 97, 116, 101,
            32, 80, 111, 108, 105, 99, 121, 32, 40, 67, 80, 41, 32, 85, 82, 76, 34, 44, 34, 67,
            101, 114, 116, 105, 102, 105, 99, 97, 116, 101, 32, 80, 114, 97, 99, 116, 105, 99, 101,
            32, 83, 116, 97, 116, 101, 109, 101, 110, 116, 32, 40, 67, 80, 83, 41, 32, 85, 82, 76,
            34, 44, 34, 67, 80, 47, 67, 80, 83, 32, 76, 97, 115, 116, 32, 85, 112, 100, 97, 116,
            101, 100, 32, 68, 97, 116, 101, 34, 44, 34, 84, 101, 115, 116, 32, 87, 101, 98, 115,
            105, 116, 101, 32, 85, 82, 76, 32, 45, 32, 86, 97, 108, 105, 100, 34, 44, 34, 84, 101,
            115, 116, 32, 87, 101, 98, 115, 105, 116, 101, 32, 85, 82, 76, 32, 45, 32, 69, 120,
            112, 105, 114, 101, 100, 34, 44, 34, 84, 101, 115, 116, 32, 87, 101, 98, 115, 105,
        ];
        let frame_nocheck: Vec<u8> = vec![
            40, 181, 47, 253, 96, 232, 2, 101, 10, 0, 210, 79, 48, 34, 64, 107, 156, 3, 155, 129,
            82, 186, 151, 21, 98, 13, 147, 4, 125, 50, 62, 54, 120, 201, 100, 110, 144, 107, 29,
            153, 190, 59, 209, 86, 71, 81, 4, 46, 231, 141, 32, 20, 183, 143, 54, 82, 145, 11, 211,
            228, 123, 99, 195, 25, 178, 227, 25, 111, 117, 81, 147, 29, 117, 97, 75, 37, 222, 182,
            155, 50, 192, 82, 137, 140, 104, 251, 192, 197, 150, 10, 88, 202, 121, 251, 156, 101,
            121, 7, 213, 194, 88, 54, 20, 198, 192, 61, 133, 181, 89, 58, 184, 252, 245, 214, 69,
            246, 59, 244, 214, 102, 11, 211, 100, 58, 86, 247, 180, 97, 65, 70, 205, 67, 202, 50,
            246, 186, 71, 210, 111, 57, 226, 59, 111, 15, 146, 96, 64, 64, 145, 32, 176, 132, 164,
            248, 104, 109, 150, 15, 110, 107, 131, 14, 103, 18, 146, 30, 69, 48, 178, 223, 89, 87,
            66, 210, 33, 85, 200, 183, 182, 117, 108, 243, 85, 66, 210, 34, 33, 29, 195, 118, 38,
            223, 24, 58, 226, 31, 101, 9, 73, 252, 247, 176, 144, 137, 74, 1, 56, 32, 16, 230, 48,
            90, 29, 32, 185, 224, 86, 3, 197, 150, 41, 143, 229, 176, 36, 147, 76, 40, 247, 101,
            65, 7, 112, 217, 204, 175, 133, 11, 93, 78, 85, 211, 128, 166, 13, 44, 106, 80, 37, 48,
            74, 3, 32, 129, 38, 27, 60, 9, 136, 140, 36, 162, 12, 4, 26, 8, 209, 16, 60, 0, 240,
            12, 231, 3, 188, 51, 176, 114, 32, 192, 0, 130, 114, 209, 13, 0, 136, 128, 62, 133, 94,
            3, 64, 38, 12, 26, 12, 103, 16, 11, 192, 232, 96, 146, 24, 230, 198, 203, 61, 117, 119,
            192, 148, 1, 164, 19, 171, 113, 26, 193, 120, 34, 52, 28, 10, 248, 47, 255, 101, 212,
            60, 91, 155, 179, 45, 69, 32, 3, 77, 107, 1, 5,
        ];
        let frame_check: Vec<u8> = vec![
            40, 181, 47, 253, 100, 232, 2, 101, 10, 0, 210, 79, 48, 34, 64, 107, 156, 3, 155, 129,
            82, 186, 151, 21, 98, 13, 147, 4, 125, 50, 62, 54, 120, 201, 100, 110, 144, 107, 29,
            153, 190, 59, 209, 86, 71, 81, 4, 46, 231, 141, 32, 20, 183, 143, 54, 82, 145, 11, 211,
            228, 123, 99, 195, 25, 178, 227, 25, 111, 117, 81, 147, 29, 117, 97, 75, 37, 222, 182,
            155, 50, 192, 82, 137, 140, 104, 251, 192, 197, 150, 10, 88, 202, 121, 251, 156, 101,
            121, 7, 213, 194, 88, 54, 20, 198, 192, 61, 133, 181, 89, 58, 184, 252, 245, 214, 69,
            246, 59, 244, 214, 102, 11, 211, 100, 58, 86, 247, 180, 97, 65, 70, 205, 67, 202, 50,
            246, 186, 71, 210, 111, 57, 226, 59, 111, 15, 146, 96, 64, 64, 145, 32, 176, 132, 164,
            248, 104, 109, 150, 15, 110, 107, 131, 14, 103, 18, 146, 30, 69, 48, 178, 223, 89, 87,
            66, 210, 33, 85, 200, 183, 182, 117, 108, 243, 85, 66, 210, 34, 33, 29, 195, 118, 38,
            223, 24, 58, 226, 31, 101, 9, 73, 252, 247, 176, 144, 137, 74, 1, 56, 32, 16, 230, 48,
            90, 29, 32, 185, 224, 86, 3, 197, 150, 41, 143, 229, 176, 36, 147, 76, 40, 247, 101,
            65, 7, 112, 217, 204, 175, 133, 11, 93, 78, 85, 211, 128, 166, 13, 44, 106, 80, 37, 48,
            74, 3, 32, 129, 38, 27, 60, 9, 136, 140, 36, 162, 12, 4, 26, 8, 209, 16, 60, 0, 240,
            12, 231, 3, 188, 51, 176, 114, 32, 192, 0, 130, 114, 209, 13, 0, 136, 128, 62, 133, 94,
            3, 64, 38, 12, 26, 12, 103, 16, 11, 192, 232, 96, 146, 24, 230, 198, 203, 61, 117, 119,
            192, 148, 1, 164, 19, 171, 113, 26, 193, 120, 34, 52, 28, 10, 248, 47, 255, 101, 212,
            60, 91, 155, 179, 45, 69, 32, 3, 77, 107, 1, 5, 100, 131, 183, 217,
        ];
        let dec = crate::decompress(&frame_nocheck, 0).expect("decode no-check frame");
        assert_eq!(dec, input);
        let dec = crate::decompress(&frame_check, 0).expect("decode checksum frame");
        assert_eq!(dec, input);
    }

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

    /// Regression (BUGREPORT-zstd-315-residual, issue #315): the
    /// 8-symbol batching in `HuffmanDecoder::decode_into` over-consumed
    /// the 64-bit container — when eight code lengths summed past the
    /// bits available after a reload, the trailing symbols of the batch
    /// peeked stale (shift-wrapped) bits and mis-decoded. This 163-byte
    /// input (binary header + long literal text + binary tail, zero
    /// sequences, all-literal block at Fastest..Better) produced exactly
    /// one wrong literal at index 143 while the system zstd CLI decoded
    /// the frame correctly. Fixed by matching C `HUF_decodeStreamX1`:
    /// at most 4 symbols between reloads, and only while reload reports
    /// Unfinished.
    #[test]
    fn issue_315_residual_round_trips_all_levels() {
        let input: Vec<u8> = vec![
            47, 8, 206, 24, 1, 0, 0, 0, 4, 165, 0, 0, 0, 100, 117, 112, 108, 105, 99, 97, 116, 101,
            32, 105, 110, 108, 105, 110, 101, 32, 99, 111, 110, 116, 101, 110, 104, 101, 32, 115,
            97, 109, 101, 32, 50, 48, 48, 45, 105, 115, 104, 32, 98, 121, 116, 101, 115, 32, 105,
            110, 32, 116, 104, 114, 101, 101, 32, 102, 105, 108, 101, 115, 44, 32, 115, 111, 32,
            116, 104, 101, 32, 119, 114, 105, 116, 101, 114, 39, 101, 115, 32, 111, 110, 32, 101,
            118, 101, 114, 121, 32, 114, 101, 97, 108, 105, 115, 116, 105, 99, 32, 116, 114, 101,
            101, 46, 32, 80, 97, 100, 0, 0, 0, 5, 233, 64, 129, 47, 8, 206, 1, 0, 0, 0, 0, 210,
            127, 239, 79, 204, 13, 133, 191, 196, 106, 114, 104, 141, 228, 107, 113, 253, 249, 246,
            166, 238, 75, 8, 35, 98, 41, 201, 222, 1,
        ];
        for level in [
            ZstdLevel::Fastest,
            ZstdLevel::Fast,
            ZstdLevel::Default,
            ZstdLevel::Better,
            ZstdLevel::Best,
        ] {
            let compressed = compress(&input, level).expect("encode");
            let decompressed = decompress(&compressed, input.len() as u32).expect("decode");
            assert_eq!(decompressed, input, "round-trip mismatch at {level:?}");
        }
    }

    /// Self-round-trip over deterministic mixed text+binary corpora at
    /// every level. Size sweeps over uniform inputs don't exercise the
    /// regime where matches/reps fire mid-literal; these shapes (binary
    /// head + literal text + binary tail, random byte soup, text with
    /// scattered high bytes) do.
    #[test]
    fn mixed_content_self_round_trips_all_levels() {
        // xorshift32 — deterministic, no RNG crate, stable across runs.
        fn next(state: &mut u32) -> u32 {
            *state ^= *state << 13;
            *state ^= *state >> 17;
            *state ^= *state << 5;
            *state
        }
        let mut state = 0xC0FFEE_u32;
        let mut corpus: Vec<Vec<u8>> = Vec::new();
        for len in [20_usize, 63, 100, 163, 200, 333, 512, 1024, 4096] {
            // Shape 1: binary head + literal text + binary tail.
            let head = (0..8).map(|_| next(&mut state) as u8).collect::<Vec<_>>();
            const TEXT: &[u8] =
                b"duplicate inline content same 200-ish bytes in three files, so the writer's on every realistic tree. Pad ";
            let text = (0..len).map(|i| TEXT[i % TEXT.len()]).collect::<Vec<_>>();
            let tail = (0..16).map(|_| next(&mut state) as u8).collect::<Vec<_>>();
            corpus.push([head, text, tail].concat());
            // Shape 2: pure random bytes (high-entropy literals).
            corpus.push((0..len).map(|_| next(&mut state) as u8).collect());
            // Shape 3: mostly-text with scattered high bytes.
            corpus.push(
                (0..len)
                    .map(|i| {
                        const T: &[u8] = b"abcdef ghijkl mnopqr stuvwx yz0123 456789 ABCDEF GHIJKL";
                        if next(&mut state) % 16 == 0 {
                            0x80 | (next(&mut state) as u8 & 0x7F)
                        } else {
                            T[i % T.len()]
                        }
                    })
                    .collect(),
            );
        }
        for input in &corpus {
            for level in [
                ZstdLevel::Fastest,
                ZstdLevel::Fast,
                ZstdLevel::Default,
                ZstdLevel::Better,
                ZstdLevel::Best,
            ] {
                let compressed = compress(input, level)
                    .unwrap_or_else(|e| panic!("encode {level:?} len {}: {e}", input.len()));
                let decompressed = decompress(&compressed, input.len() as u32)
                    .unwrap_or_else(|e| panic!("decode {level:?} len {}: {e}", input.len()));
                assert_eq!(
                    decompressed,
                    *input,
                    "round-trip mismatch at {level:?} len {}",
                    input.len()
                );
            }
        }
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
            let compressed = compressor
                .compress(input, ZstdLevel::Default)
                .expect("compress");
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
        let small_out = compressor
            .compress(small, ZstdLevel::Default)
            .expect("small");
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
        let decompressed = decompress_with_dict(&compressed, sample.len() as u32, &dict)
            .expect("decode with dict");
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
        let sample =
            b"function handler_X() { return CONSTANT_PREFIX_X + SHARED_SUFFIX; }\n".to_vec();

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
        assert!(
            did_flag != 0,
            "expected Dictionary_ID_flag != 0, got {did_flag}"
        );

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
        let decompressed =
            decompress_with_dict(&compressed, sample.len() as u32, &dict).expect("decode");
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
        let compressed = super::compress(&input, super::ZstdLevel::Fast).expect("compress");

        // Decompress and verify round-trip.
        let decompressed = decompress(&compressed, input.len() as u32).expect("decompress");

        assert_eq!(
            decompressed, input,
            "Round-trip failed for FSE-compressed Huffman weights input"
        );
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
            0x1e, 0x10, 0xd8, 0xda, 0x72, 0x0c, 0x03, 0xb8, 0xa2, 0x61, 0x70, 0x4d, 0x92, 0x3a,
            0x91, 0x6e, 0xa1, 0x26, 0x12, 0xd9, 0x6e, 0xa1, 0xa5, 0x95, 0xed, 0x16, 0x35, 0x0c,
            0x53, 0x91, 0x02,
        ];

        let (table, consumed) = crate::huffman::weights::read_huffman_table(fse_header)
            .expect("read FSE-compressed Huffman table");

        // The table should have 256 symbols (255 FSE-decoded + 1 implied).
        assert_eq!(
            table.symbol_count(),
            256,
            "Huffman table should have 256 symbols"
        );
        assert_eq!(
            consumed, 31,
            "Should consume all 31 bytes (tree byte + 30 FSE bytes)"
        );

        // Verify the weights match the C reference output.
        let weights = table.weights();
        assert_eq!(weights[0], 8, "symbol 0 weight");
        assert_eq!(weights[142], 2, "symbol 142 weight");
        assert_eq!(weights[195], 2, "symbol 195 weight");
        assert_eq!(weights[212], 2, "symbol 212 weight");
        assert_eq!(weights[255], 1, "implied last weight");
    }
}
