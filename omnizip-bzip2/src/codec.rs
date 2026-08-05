//! `BZip2` codec — the [`Codec`] implementation that orchestrates the full
//! `BZip2` pipeline.
//!
//! Port of `omnizip/lib/omnizip/algorithms/bzip2/{encoder,decoder}.rb`.
//!
//! ## Container format (matches the Ruby reference)
//!
//! Each block is laid out as:
//!
//! ```text
//!   u32  crc32           (of the original plaintext block)
//!   u32  primary_index   (BWT primary index)
//!   u32  original_len    (original block length, in bytes)
//!   u32  rle_len         (length of the RLE1 stream, in symbols)
//!   u16  code_count      (number of Huffman code-length entries)
//!   [u8 symbol, u8 code_length] * code_count
//!   u8   padding_bits    (0..=7 trailing zero bits in the bit stream)
//!   u32  bitstream_len   (bytes of packed Huffman bit stream)
//!   [u8] * bitstream_len
//! ```
//!
//! Multiple blocks are concatenated; an empty input produces an empty output.
//! All integers are big-endian (matching Ruby's `pack("N")` / `pack("n")`).

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

use crate::bwt::{bwt_decode, bwt_encode};
use crate::crc32::crc32;
use crate::huffman::{build_code_lengths, huffman_decode, huffman_encode, CodeLengths, FreqTable};
use crate::mtf::{mtf_decode, mtf_encode};
use crate::rle::{rle_decode, rle_encode};

/// Minimum supported block size — Ruby `MIN_BLOCK_SIZE`.
pub const MIN_BLOCK_SIZE: usize = 100_000;

/// Maximum supported block size — Ruby `MAX_BLOCK_SIZE`.
pub const MAX_BLOCK_SIZE: usize = 900_000;

/// Default block size — Ruby `DEFAULT_BLOCK_SIZE`.
#[allow(dead_code)]
pub const DEFAULT_BLOCK_SIZE: usize = 900_000;

/// `BZip2` codec. Block size is derived from the compression level: level 1
/// maps to 100 KB, level 9 to 900 KB, linearly in between. Levels outside
/// `1..=9` are rejected.
///
/// For an explicit block size, use [`Bzip2Codec::compress_with_block_size`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Bzip2Codec;

impl Bzip2Codec {
    /// Construct a new `Bzip2Codec`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Map a level in `1..=9` to a block size in bytes.
    pub fn block_size_for(level: u8) -> usize {
        // Level 1 -> 100_000, level 9 -> 900_000. Step is 100_000 per level.
        let scaled = usize::from(level).clamp(1, 9);
        scaled * 100_000
    }

    /// Compress with an explicit block size in bytes (100_000..=900_000,
    /// in 100 KB increments matching the upstream `bzip2 -1`..`-9` flags).
    ///
    /// Smaller blocks encode faster but compress worse; larger blocks
    /// compress better but use more memory and time. Memory peak is
    /// ~5x block size (BWT buffers).
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::LevelOutOfRange`] if `block_size` is not
    /// a valid BZip2 block size (multiple of 100_000 in 100_000..=900_000).
    pub fn compress_with_block_size(
        &self,
        plaintext: &[u8],
        block_size: usize,
    ) -> Result<Vec<u8>, OmnizipError> {
        if block_size < MIN_BLOCK_SIZE
            || block_size > MAX_BLOCK_SIZE
            || block_size % 100_000 != 0
        {
            return Err(OmnizipError::LevelOutOfRange {
                codec: CodecId::BZIP2,
                level: 0,
                min: 1,
                max: 9,
            });
        }
        if plaintext.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for chunk in plaintext.chunks(block_size) {
            encode_block(chunk, &mut out);
        }
        Ok(out)
    }
}

impl Codec for Bzip2Codec {
    fn id(&self) -> CodecId {
        CodecId::BZIP2
    }

    fn name(&self) -> &'static str {
        "bzip2"
    }

    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        let lv = level.as_u8();
        if !(1..=9).contains(&lv) {
            return Err(OmnizipError::LevelOutOfRange {
                codec: CodecId::BZIP2,
                level: lv,
                min: 1,
                max: 9,
            });
        }

        if plaintext.is_empty() {
            return Ok(Vec::new());
        }

        let block_size = Self::block_size_for(lv);

        let mut out = Vec::new();
        for chunk in plaintext.chunks(block_size) {
            encode_block(chunk, &mut out);
        }
        Ok(out)
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let expected_us = usize::try_from(expected_len).map_err(|_| OmnizipError::Corrupt {
            codec: CodecId::BZIP2,
            reason: format!("expected_len {expected_len} exceeds usize"),
        })?;

        if compressed.is_empty() {
            if expected_us == 0 {
                return Ok(Vec::new());
            }
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::BZIP2,
                expected: expected_len,
                actual: 0,
            });
        }

        let mut out = Vec::with_capacity(expected_us);
        let mut cursor = 0usize;
        while cursor < compressed.len() {
            let (block, consumed) = decode_block(&compressed[cursor..])?;
            out.extend_from_slice(&block);
            cursor += consumed;
        }

        if out.len() != expected_us {
            return Err(OmnizipError::LengthMismatch {
                codec: CodecId::BZIP2,
                expected: expected_len,
                actual: out.len(),
            });
        }
        Ok(out)
    }
}

/// Reusable BZip2 compressor that caches the output Vec across calls.
///
/// BZip2's `bwt_encode` allocates suffix-array buffers internally
/// per call; the only externally-poolable allocation is the output
/// `Vec<u8>` and the per-block MTF/RLE scratch. This struct pools
/// the output buffer (most relevant for batch workloads with many
/// small inputs).
///
/// ## Example
///
/// ```no_run
/// # let paths: [&str; 0] = [];
/// use omnizip_bzip2::Bzip2Compressor;
/// use omnizip_codecs::CompressionLevel;
///
/// let mut comp = Bzip2Compressor::new();
/// for input in paths {
///     let bytes = std::fs::read(input).unwrap();
///     let encoded = comp.compress(&bytes, CompressionLevel::default()).unwrap();
///     // ... use encoded
/// }
/// ```
pub struct Bzip2Compressor {
    /// Cached output buffer. Cleared (not freed) per call.
    out: Vec<u8>,
}

impl Default for Bzip2Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Bzip2Compressor {
    /// Construct a reusable BZip2 compressor with empty buffers.
    #[must_use]
    pub const fn new() -> Self {
        Self { out: Vec::new() }
    }

    /// Compress `input` at the given level, reusing the cached output
    /// buffer. Output is byte-identical to [`Bzip2Codec::compress`].
    pub fn compress(
        &mut self,
        input: &[u8],
        level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        let lv = level.as_u8();
        if !(1..=9).contains(&lv) {
            return Err(OmnizipError::LevelOutOfRange {
                codec: CodecId::BZIP2,
                level: lv,
                min: 1,
                max: 9,
            });
        }
        if input.is_empty() {
            self.out.clear();
            return Ok(Vec::new());
        }
        let block_size = Bzip2Codec::block_size_for(lv);
        self.out.clear();
        for chunk in input.chunks(block_size) {
            encode_block(chunk, &mut self.out);
        }
        // Clone out for the caller; we keep the capacity for the next call.
        Ok(self.out.clone())
    }
}

/// Encode a single block and append the wire-format bytes to `out`.
fn encode_block(block: &[u8], out: &mut Vec<u8>) {
    // 1. CRC of original data.
    let crc = crc32(block);

    // 2. BWT.
    let (bwt_data, primary_index) = bwt_encode(block);

    // 3. MTF.
    let mtf_data = mtf_encode(&bwt_data);

    // 4. RLE1 (the Ruby applies RLE after MTF — the data is mostly zeros at
    //    this point so RLE compresses well).
    let rle_data = rle_encode(&mtf_data);
    let rle_len = rle_data.len();

    // 5. Build frequency table + Huffman code lengths.
    let mut freqs = FreqTable::new();
    for &b in &rle_data {
        *freqs.entry(b).or_insert(0) += 1;
    }
    let lengths = build_code_lengths(&freqs);

    // 6. Huffman-encode the RLE stream.
    let (bitstream, padding) = huffman_encode(&rle_data, &lengths);

    // --- Write wire format ---
    out.extend_from_slice(&crc.to_be_bytes());
    out.extend_from_slice(&primary_index.to_be_bytes());
    out.extend_from_slice(&(block.len() as u32).to_be_bytes());
    out.extend_from_slice(&(rle_len as u32).to_be_bytes());

    // Code table.
    out.extend_from_slice(&(lengths.len() as u16).to_be_bytes());
    for (&symbol, &len) in &lengths {
        out.push(symbol);
        out.push(len);
    }

    // Bit stream.
    out.push(padding);
    out.extend_from_slice(&(bitstream.len() as u32).to_be_bytes());
    out.extend_from_slice(&bitstream);
}

/// Decode a single block starting at the beginning of `buf`.
///
/// Returns `(decoded_bytes, bytes_consumed)`.
fn decode_block(buf: &[u8]) -> Result<(Vec<u8>, usize), OmnizipError> {
    let mut pos = 0usize;

    let crc = read_u32(buf, &mut pos)?;
    let primary_index = read_u32(buf, &mut pos)?;
    let original_len = read_u32(buf, &mut pos)? as usize;
    let rle_len = read_u32(buf, &mut pos)? as usize;

    let code_count = read_u16(buf, &mut pos)? as usize;
    let mut lengths = CodeLengths::new();
    for _ in 0..code_count {
        let symbol = read_u8(buf, &mut pos)?;
        let len = read_u8(buf, &mut pos)?;
        lengths.insert(symbol, len);
    }

    let padding = read_u8(buf, &mut pos)?;
    let bitstream_len = read_u32(buf, &mut pos)? as usize;
    let bitstream = read_bytes(buf, &mut pos, bitstream_len)?;

    // Reverse pipeline.
    let rle_data = huffman_decode(bitstream, &lengths, rle_len, padding).map_err(|e| {
        OmnizipError::DecodeFailed {
            codec: CodecId::BZIP2,
            reason: format!("huffman decode failed: {e}"),
        }
    })?;

    let mtf_data = rle_decode(&rle_data).map_err(|e| OmnizipError::DecodeFailed {
        codec: CodecId::BZIP2,
        reason: format!("rle decode failed: {e}"),
    })?;

    let bwt_data = mtf_decode(&mtf_data);

    let original =
        bwt_decode(&bwt_data, primary_index).map_err(|e| OmnizipError::DecodeFailed {
            codec: CodecId::BZIP2,
            reason: format!("bwt decode failed: {e}"),
        })?;

    if original.len() != original_len {
        return Err(OmnizipError::DecodeFailed {
            codec: CodecId::BZIP2,
            reason: format!(
                "block length mismatch: header says {original_len}, decoded {}",
                original.len()
            ),
        });
    }

    let actual_crc = crc32(&original);
    if actual_crc != crc {
        return Err(OmnizipError::DecodeFailed {
            codec: CodecId::BZIP2,
            reason: format!("CRC mismatch: expected {crc:#010x}, got {actual_crc:#010x}"),
        });
    }

    Ok((original, pos))
}

// --- helpers for big-endian reads with bounds checking ---

fn read_u32(buf: &[u8], pos: &mut usize) -> Result<u32, OmnizipError> {
    if *pos + 4 > buf.len() {
        return Err(truncated("u32"));
    }
    let bytes: [u8; 4] = [buf[*pos], buf[*pos + 1], buf[*pos + 2], buf[*pos + 3]];
    *pos += 4;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u16(buf: &[u8], pos: &mut usize) -> Result<u16, OmnizipError> {
    if *pos + 2 > buf.len() {
        return Err(truncated("u16"));
    }
    let bytes: [u8; 2] = [buf[*pos], buf[*pos + 1]];
    *pos += 2;
    Ok(u16::from_be_bytes(bytes))
}

fn read_u8(buf: &[u8], pos: &mut usize) -> Result<u8, OmnizipError> {
    if *pos >= buf.len() {
        return Err(truncated("u8"));
    }
    let v = buf[*pos];
    *pos += 1;
    Ok(v)
}

fn read_bytes<'a>(buf: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], OmnizipError> {
    if *pos + n > buf.len() {
        return Err(truncated("byte slice"));
    }
    let v = &buf[*pos..*pos + n];
    *pos += n;
    Ok(v)
}

fn truncated(what: &str) -> OmnizipError {
    OmnizipError::Corrupt {
        codec: CodecId::BZIP2,
        reason: format!("truncated input while reading {what}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_out_of_range_levels() {
        let codec = Bzip2Codec::new();
        assert!(codec.compress(b"x", CompressionLevel::new(0)).is_err());
        assert!(codec.compress(b"x", CompressionLevel::new(10)).is_err());
    }

    #[test]
    fn block_size_scales_with_level() {
        assert_eq!(Bzip2Codec::block_size_for(1), 100_000);
        assert_eq!(Bzip2Codec::block_size_for(9), 900_000);
        assert_eq!(Bzip2Codec::block_size_for(5), 500_000);
    }

    #[test]
    fn reusable_compressor_matches_one_shot() {
        let input: Vec<u8> = b"the quick brown fox jumps over the lazy dog. ".repeat(50);
        let one_shot = Bzip2Codec::new()
            .compress(&input, CompressionLevel::default())
            .expect("one-shot");
        let mut reusable = Bzip2Compressor::new();
        let reusable_out = reusable
            .compress(&input, CompressionLevel::default())
            .expect("reusable");
        assert_eq!(one_shot, reusable_out);
    }

    #[test]
    fn reusable_compressor_round_trips_across_calls() {
        let mut comp = Bzip2Compressor::new();
        for input in [
            b"foo".as_ref(),
            b"hello world hello world".as_ref(),
            b"the quick brown fox jumps over the lazy dog. ".repeat(20).as_slice(),
        ] {
            let encoded = comp.compress(input, CompressionLevel::default()).expect("compress");
            let decoded = Bzip2Codec::new()
                .decompress(&encoded, input.len() as u32)
                .expect("decode");
            assert_eq!(decoded.as_slice(), input);
        }
    }
}
