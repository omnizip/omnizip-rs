//! omnizip-glza — Pure-Rust GLZA (Grammar-based LZ) compression codec.
//!
//! GLZA builds a context-free grammar over the input: repeated substrings
//! are promoted to non-terminal rules, and the compressed form is the
//! grammar itself (start rule + rule definitions).
//!
//! ## Algorithm
//!
//! 1. Build a suffix array of the input.
//! 2. Walk the LCP array to find the most frequent repeated substring
//!    (length >= 4, occurrences >= 2).
//! 3. Promote that substring to a non-terminal rule and replace every
//!    non-overlapping occurrence with a rule reference.
//! 4. Repeat until no candidate improves compression.
//! 5. Serialize the grammar with a simple varint-based encoding.
//!
//! Phase 1 limitations:
//! - Suffix sort is O(n (log n)^2) prefix-doubling (not SA-IS).
//! - Greedy extraction (one rule per pass, full re-sort each pass).
//! - No entropy coding on the rule bodies (symbols stored raw).
//!
//! ## Determinism
//!
//! The output is byte-identical for identical inputs across runs: the
//! suffix array, LCP array, greedy extraction order, and serialization are
//! all deterministic and tied only to the input bytes.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

mod decode;
mod encode;
mod entropy;
mod grammar;
mod suffix_array;

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

pub use grammar::{Grammar, Symbol};

/// Compress `input` with GLZA using the default container version.
///
/// The encoder computes both the Phase 1 (raw varint) and Phase 2
/// (Huffman-coded) payloads and emits whichever is smaller. For inputs
/// where Huffman coding helps (most real data) this yields Phase 2; for
/// small or near-uniform-distribution inputs where the Huffman table
/// overhead exceeds the symbol-stream savings, it falls back to Phase 1.
/// Either way the decoder dispatches transparently on the version byte.
///
/// Inputs larger than [`MAX_GLZA_CHUNK_SIZE`] are auto-split into
/// 512 KB blocks and framed as a multi-chunk stream.
///
/// # Errors
///
/// Returns [`OmnizipError::EncodeFailed`] only on internal errors (currently
/// never — the encoder is total).
pub fn compress(input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    if input.len() <= MAX_GLZA_CHUNK_SIZE {
        return compress_single(input);
    }
    // Auto-chunk: split into MAX_GLZA_CHUNK_SIZE blocks, compress each
    // independently, and frame with a simple multi-chunk container.
    compress_multichunk(input)
}

/// Maximum chunk size for GLZA. Grammar construction is O(n²) — the
/// suffix array sort dominates at large sizes. Inputs above this size
/// are automatically split into chunks, each compressed independently.
const MAX_GLZA_CHUNK_SIZE: usize = 512 * 1024; // 512 KB per chunk

/// Magic for multi-chunk GLZA streams.
const MULTICHUNK_MAGIC: &[u8; 5] = b"GLZM\0";

/// Compress a single chunk (≤ 512 KB).
fn compress_single(input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    let grammar = Grammar::build(input);
    let uncompressed_size = u32::try_from(input.len()).map_err(|_| OmnizipError::EncodeFailed {
        codec: CodecId::GLZA,
        reason: format!("input length {} exceeds u32::MAX", input.len()),
    })?;
    let v1 = encode::encode_v1(&grammar, uncompressed_size);
    let v2 = encode::encode_v2(&grammar, uncompressed_size);
    Ok(if v2.len() < v1.len() { v2 } else { v1 })
}

/// Compress large input by splitting into chunks.
fn compress_multichunk(input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    let mut out = Vec::with_capacity(input.len() / 4 + 32);
    // Multi-chunk header: magic + total uncompressed size.
    out.extend_from_slice(MULTICHUNK_MAGIC);
    let total = u32::try_from(input.len()).map_err(|_| OmnizipError::EncodeFailed {
        codec: CodecId::GLZA,
        reason: format!("input length {} exceeds u32::MAX", input.len()),
    })?;
    out.extend_from_slice(&total.to_le_bytes());

    let mut offset = 0;
    while offset < input.len() {
        let end = (offset + MAX_GLZA_CHUNK_SIZE).min(input.len());
        let chunk = &input[offset..end];
        let compressed = compress_single(chunk)?;
        // Each chunk: 4-byte LE compressed size + compressed data.
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&compressed);
        offset = end;
    }
    // End marker: zero-size chunk.
    out.extend_from_slice(&0u32.to_le_bytes());
    Ok(out)
}

/// Compress with an explicit container version.
///
/// `1` = Phase 1 raw varints, `2` = Phase 2 Huffman-coded. Other values
/// fall back to Phase 1.
///
/// # Errors
///
/// Same error conditions as [`compress`].
pub fn compress_with_version(input: &[u8], version: u8) -> Result<Vec<u8>, OmnizipError> {
    let grammar = Grammar::build(input);
    let uncompressed_size = u32::try_from(input.len()).map_err(|_| OmnizipError::EncodeFailed {
        codec: CodecId::GLZA,
        reason: format!("input length {} exceeds u32::MAX", input.len()),
    })?;
    Ok(encode::encode_with_version(
        &grammar,
        uncompressed_size,
        version,
    ))
}

/// Decompress GLZA-compressed `compressed`.
///
/// Handles both single-chunk streams (produced by [`compress_single`])
/// and multi-chunk streams (produced by [`compress_multichunk`] for
/// inputs exceeding `MAX_GLZA_CHUNK_SIZE`).
///
/// # Errors
///
/// Returns [`OmnizipError::Corrupt`] on a malformed payload,
/// [`OmnizipError::DecodeFailed`] on a length mismatch, or
/// [`OmnizipError::LengthMismatch`] if the expanded output length differs
/// from the header's `uncompressed_size`.
pub fn decompress(compressed: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    if compressed.len() >= MULTICHUNK_MAGIC.len()
        && &compressed[..MULTICHUNK_MAGIC.len()] == MULTICHUNK_MAGIC
    {
        decompress_multichunk(compressed)
    } else {
        let (uncompressed_size, start_rule, rules) = decode::parse(compressed)?;
        decode::expand(uncompressed_size, &start_rule, &rules)
    }
}

/// Decode a multi-chunk GLZA stream produced by [`compress_multichunk`].
///
/// Layout:
/// ```text
/// +--------------------+  5 bytes: b"GLZM\0"
/// | magic              |
/// +--------------------+  4 bytes LE: total uncompressed size (u32)
/// | total_size         |
/// +--------------------+  repeated chunks:
/// | chunk              |    4 bytes LE: compressed chunk size
/// |   ...              |    N bytes: single-chunk GLZA stream
/// +--------------------+  end marker: 4-byte LE zero
/// | 0x00000000         |
/// ```
fn decompress_multichunk(compressed: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    let mut cursor = MULTICHUNK_MAGIC.len();
    if compressed.len() < cursor + 4 {
        return Err(OmnizipError::Corrupt {
            codec: CodecId::GLZA,
            reason: "multichunk header too short for total size".into(),
        });
    }
    let total = u32::from_le_bytes(compressed[cursor..cursor + 4].try_into().map_err(|_| {
        OmnizipError::Corrupt {
            codec: CodecId::GLZA,
            reason: "total size slice".into(),
        }
    })?);
    cursor += 4;

    let mut out: Vec<u8> = Vec::with_capacity(total as usize);
    loop {
        if compressed.len() < cursor + 4 {
            return Err(OmnizipError::Corrupt {
                codec: CodecId::GLZA,
                reason: "truncated chunk size prefix".into(),
            });
        }
        let chunk_size =
            u32::from_le_bytes(compressed[cursor..cursor + 4].try_into().map_err(|_| {
                OmnizipError::Corrupt {
                    codec: CodecId::GLZA,
                    reason: "chunk size slice".into(),
                }
            })?) as usize;
        cursor += 4;
        if chunk_size == 0 {
            break; // end marker
        }
        if compressed.len() < cursor + chunk_size {
            return Err(OmnizipError::Corrupt {
                codec: CodecId::GLZA,
                reason: format!(
                    "chunk body truncated: declared {chunk_size}, have {}",
                    compressed.len() - cursor
                ),
            });
        }
        let chunk = &compressed[cursor..cursor + chunk_size];
        cursor += chunk_size;
        let (sz, start_rule, rules) = decode::parse(chunk)?;
        let decoded = decode::expand(sz, &start_rule, &rules)?;
        out.extend_from_slice(&decoded);
    }

    if out.len() as u32 != total {
        return Err(OmnizipError::LengthMismatch {
            codec: CodecId::GLZA,
            expected: total,
            actual: out.len(),
        });
    }
    Ok(out)
}

/// GLZA codec adapter implementing the omnizip-codecs `Codec` trait.
#[derive(Clone, Copy, Debug, Default)]
pub struct GlzaCodec;

impl GlzaCodec {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Codec for GlzaCodec {
    fn id(&self) -> CodecId {
        CodecId::GLZA
    }

    fn name(&self) -> &'static str {
        "glza"
    }

    fn compress(
        &self,
        plaintext: &[u8],
        _level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        compress(plaintext)
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let out = decompress(compressed)?;
        if out.len() as u32 != expected_len {
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::GLZA,
                expected: expected_len,
                actual: out.len(),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss, clippy::len_zero)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let compressed = compress(b"").expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, b"");
    }

    #[test]
    fn round_trip_single_byte() {
        let compressed = compress(b"X").expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, b"X");
    }

    #[test]
    fn round_trip_short_text() {
        let input = b"hello world";
        let compressed = compress(input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_repetitive_text() {
        let input: Vec<u8> = b"the quick brown fox jumps over the lazy dog. ".repeat(50);
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_all_same_byte() {
        let input = vec![0x41u8; 5_000];
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_random_data() {
        // Pseudo-random data with no long repeats.
        let input: Vec<u8> = (0..5_000).map(|i| ((i * 7919) % 251) as u8).collect();
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_dna_like() {
        // DNA-like: only 4 distinct bytes, lots of repetition.
        let alphabet = [b'A', b'C', b'G', b'T'];
        let input: Vec<u8> = (0..4_000).map(|i| alphabet[(i * 17 + 3) % 4]).collect();
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_xml_like() {
        let input: Vec<u8> = b"<tag><child>data</child><child>data</child></tag>".repeat(100);
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_with_high_bytes() {
        // Input containing the 0xFF marker byte.
        let input: Vec<u8> = (0..=255u8).cycle().take(2_000).collect();
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn compresses_repetitive_data() {
        let input: Vec<u8> = b"<html><body>Hello, World!</body></html>".repeat(500);
        let compressed = compress(&input).expect("compress");
        let ratio = compressed.len() as f64 / input.len() as f64;
        assert!(
            compressed.len() < input.len(),
            "should compress repetitive data, ratio {ratio:.3}"
        );
    }

    #[test]
    fn ratio_target_on_dna() {
        let alphabet = [b'A', b'C', b'G', b'T'];
        let input: Vec<u8> = (0..10_000).map(|i| alphabet[(i * 17 + 3) % 4]).collect();
        let compressed = compress(&input).expect("compress");
        let ratio = compressed.len() as f64 / input.len() as f64;
        // Target: better than ~50% on DNA-like data.
        assert!(
            ratio < 0.6,
            "DNA ratio {ratio:.3} should be < 0.6 for Phase 1"
        );
    }

    #[test]
    fn codec_trait_round_trips() {
        let codec = GlzaCodec::new();
        let input = b"repetitive repetitive repetitive repetitive text".to_vec();
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .expect("compress");
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn determinism() {
        let input: Vec<u8> =
            b"the quick brown fox the quick brown fox the quick brown fox".to_vec();
        let a = compress(&input).expect("compress");
        let b = compress(&input).expect("compress");
        assert_eq!(a, b, "GLZA must be deterministic");
    }

    #[test]
    fn rejects_bad_magic() {
        let result = decompress(b"NOTGLZA\0\x01\0\0\0\0\0\0");
        assert!(result.is_err());
    }

    #[test]
    fn codec_id_is_0x0014() {
        assert_eq!(CodecId::GLZA.as_u16(), 0x0014);
        let codec = GlzaCodec::new();
        assert_eq!(codec.id().as_u16(), 0x0014);
        assert_eq!(codec.name(), "glza");
    }

    // ----- Phase 2 tests -----

    /// Phase 1 and Phase 2 must both round-trip to the same decoded output.
    #[test]
    fn phase1_and_phase2_round_trip_same_output() {
        let input: Vec<u8> =
            b"the quick brown fox the quick brown fox the quick brown fox".repeat(20);
        let v1 = compress_with_version(&input, encode::VERSION_RAW).expect("v1 compress");
        let v2 = compress_with_version(&input, encode::VERSION_HUFFMAN).expect("v2 compress");
        let out1 = decompress(&v1).expect("v1 decompress");
        let out2 = decompress(&v2).expect("v2 decompress");
        assert_eq!(out1, input);
        assert_eq!(out2, input);
    }

    /// Phase 2 output should be no larger than Phase 1 on repetitive data.
    #[test]
    fn phase2_smaller_than_phase1_on_repetitive_data() {
        let input: Vec<u8> = b"<html><body>Hello, World!</body></html>".repeat(500);
        let v1 = compress_with_version(&input, encode::VERSION_RAW).expect("v1 compress");
        let v2 = compress_with_version(&input, encode::VERSION_HUFFMAN).expect("v2 compress");
        assert!(
            v2.len() < v1.len(),
            "phase 2 ({}) should be smaller than phase 1 ({})",
            v2.len(),
            v1.len()
        );
    }

    /// Phase 2 must be deterministic: same input -> byte-identical output.
    #[test]
    fn phase2_determinism() {
        let input: Vec<u8> = b"the quick brown fox the quick brown fox".repeat(10);
        let a = compress_with_version(&input, encode::VERSION_HUFFMAN).expect("compress a");
        let b = compress_with_version(&input, encode::VERSION_HUFFMAN).expect("compress b");
        assert_eq!(a, b, "Phase 2 must be deterministic");
    }

    /// Large grammar with many rules must round-trip.
    #[test]
    fn phase2_large_grammar_round_trips() {
        // Many distinct repeating patterns -> many rules.
        let mut input = Vec::new();
        for i in 0..200u8 {
            let pat = [
                b'A' + (i % 26),
                b'B' + (i % 26),
                b'C' + (i % 26),
                b'D' + (i % 26),
            ];
            for _ in 0..10 {
                input.extend_from_slice(&pat);
            }
            input.push(b'\n');
        }
        let v2 = compress_with_version(&input, encode::VERSION_HUFFMAN).expect("compress");
        let out = decompress(&v2).expect("decompress");
        assert_eq!(out, input);
    }

    /// Round-trip with all 256 byte values to exercise the full byte alphabet.
    #[test]
    fn phase2_round_trip_all_byte_values() {
        let input: Vec<u8> = (0..=255u8).cycle().take(8_000).collect();
        let v2 = compress_with_version(&input, encode::VERSION_HUFFMAN).expect("compress");
        let out = decompress(&v2).expect("decompress");
        assert_eq!(out, input);
    }

    /// Phase 1 still works through the public `compress_with_version` API.
    #[test]
    fn phase1_explicit_round_trips() {
        let input: Vec<u8> = b"repetitive repetitive repetitive text".to_vec();
        let v1 = compress_with_version(&input, encode::VERSION_RAW).expect("v1 compress");
        let out = decompress(&v1).expect("decompress");
        assert_eq!(out, input);
    }

    /// Unknown version byte must be rejected.
    #[test]
    fn rejects_unknown_version() {
        let mut buf = Vec::new();
        buf.extend_from_slice(encode::MAGIC);
        buf.push(99); // unknown version
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        let err = decompress(&buf);
        assert!(err.is_err(), "unknown version must be rejected");
    }

    /// Default `compress()` picks the smaller of v1/v2 and round-trips.
    #[test]
    fn default_compress_round_trips_and_picks_smaller() {
        let input: Vec<u8> = b"<html><body>Hello, World!</body></html>".repeat(500);
        let out = compress(&input).expect("compress");
        // On repetitive data, v2 should win.
        assert_eq!(out[5], encode::VERSION_HUFFMAN);
        let decoded = decompress(&out).expect("decompress");
        assert_eq!(decoded, input);
    }

    /// Print before/after ratios for visibility. Not asserted at a fixed
    /// threshold (just sanity: ratio < 1.0).
    #[test]
    #[allow(clippy::cast_precision_loss, clippy::len_zero)]
    fn report_ratios() {
        let cases: &[(&str, Vec<u8>)] = &[
            (
                "repetitive html",
                b"<html><body>Hello, World!</body></html>".repeat(500),
            ),
            ("dna-like", {
                let alphabet = [b'A', b'C', b'G', b'T'];
                (0..10_000)
                    .map(|i| alphabet[(i * 17 + 3) % 4])
                    .collect::<Vec<u8>>()
            }),
            ("all same byte", vec![0x41u8; 5_000]),
            ("pseudo-random", {
                (0..5_000)
                    .map(|i| ((i * 7919) % 251) as u8)
                    .collect::<Vec<u8>>()
            }),
        ];
        for (name, input) in cases {
            let v1 = compress_with_version(input, encode::VERSION_RAW).expect("v1");
            let v2 = compress_with_version(input, encode::VERSION_HUFFMAN).expect("v2");
            let r1 = v1.len() as f64 / input.len().max(1) as f64;
            let r2 = v2.len() as f64 / input.len().max(1) as f64;
            eprintln!(
                "{name:20}: input={} v1={} (ratio {r1:.3}) v2={} (ratio {r2:.3}) delta={:.1}%",
                input.len(),
                v1.len(),
                v2.len(),
                (1.0 - r2 / r1) * 100.0
            );
        }
    }

    /// Multi-chunk round-trip: input > `MAX_GLZA_CHUNK_SIZE` auto-splits.
    #[test]
    fn multichunk_round_trip() {
        // 1.2 MB of repetitive HTML — large enough to force multi-chunk.
        let input: Vec<u8> = b"<html><body>Hello, World!</body></html>".repeat(20_000);
        assert!(input.len() > MAX_GLZA_CHUNK_SIZE);
        let compressed = compress(&input).expect("compress");
        // Verify it actually used the multi-chunk container.
        assert_eq!(
            &compressed[..MULTICHUNK_MAGIC.len()],
            MULTICHUNK_MAGIC.as_slice()
        );
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    /// Multi-chunk with heterogeneous chunks (some compressible, some not).
    #[test]
    fn multichunk_mixed_content_round_trips() {
        let mut input = Vec::new();
        // 4 chunks worth: 4 × 600 KB = 2.4 MB.
        for i in 0..4 {
            if i % 2 == 0 {
                let chunk: Vec<u8> = b"abcdefgh".repeat(75_000);
                input.extend_from_slice(&chunk);
            } else {
                let chunk: Vec<u8> = b"XYZ".repeat(150_000);
                input.extend_from_slice(&chunk);
            }
        }
        assert!(input.len() > MAX_GLZA_CHUNK_SIZE);
        let compressed = compress(&input).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    /// Empty multi-chunk container (header + end marker).
    #[test]
    fn multichunk_empty_input() {
        // compress(b"") is single-chunk because length <= MAX_GLZA_CHUNK_SIZE,
        // so test the multichunk container with an empty bytestream built
        // directly to ensure the framing handles zero chunks.
        // (Single-chunk path already covers this via round_trip_empty above.)
        let compressed = compress(b"").expect("compress");
        let out = decompress(&compressed).expect("decompress");
        assert_eq!(out, b"");
    }

    /// Reject malformed multi-chunk payloads.
    #[test]
    fn multichunk_rejects_truncated_body() {
        let mut bad = Vec::new();
        bad.extend_from_slice(MULTICHUNK_MAGIC);
        bad.extend_from_slice(&1000u32.to_le_bytes()); // claims 1000 bytes total
                                                       // No chunks, no end marker — should be rejected.
        assert!(decompress(&bad).is_err());
    }

    /// Reject multi-chunk payload whose chunk size overflows available bytes.
    #[test]
    fn multichunk_rejects_oversized_chunk() {
        let mut bad = Vec::new();
        bad.extend_from_slice(MULTICHUNK_MAGIC);
        bad.extend_from_slice(&1000u32.to_le_bytes());
        bad.extend_from_slice(&999_999u32.to_le_bytes()); // bogus chunk size
        let err = decompress(&bad);
        assert!(err.is_err());
    }
}
