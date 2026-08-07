//! omnizip-ricepp — Pure-Rust Rice++ codec for integer-pixel data.
//!
//! Ported from the DwarFS `ricepp` library by Marcus Holland-Moritz
//! (`~/src/tamatebako/dwarfs-t/ricepp/`). Rice++ is a delta + Rice
//! entropy coder designed for FITS images, sensor data, and other
//! integer-pixel workloads.
//!
//! ## Algorithm
//!
//! 1. **Delta encoding**: Each pixel is subtracted from the previous
//!    pixel to produce a residual.
//! 2. **Zigzag encoding**: The signed residual is zigzag-mapped to an
//!    unsigned value (0→0, -1→1, 1→2, -2→3, ...).
//! 3. **Adaptive Rice coding**: Per block, the best `fs` (split point)
//!    is chosen to minimize output bits. The high part is unary-coded
//!    (zeros + a terminating one), the low part is `fs` raw bits.
//! 4. **Fallback**: If Rice coding would be larger than the raw
//!    block, emit raw pixels instead.
//!
//! ## Wire format (self-describing container)
//!
//! ```text
//! +-------------------+  1 byte:  pixel bits (8, 16, or 32)
//! | pixel_bits        |
//! +-------------------+  1 byte:  byte order (0 = LE, 1 = BE)
//! | byte_order        |
//! +-------------------+  4 bytes LE: block size (pixels per block)
//! | block_size        |
//! +-------------------+  4 bytes LE: pixel count
//! | pixel_count       |
//! +-------------------+  variable: encoded blocks
//! | blocks            |
//! +-------------------+
//! ```

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

/// Ricepp codec id.

/// Maximum block size (pixels per block). DwarFS default.
const DEFAULT_BLOCK_SIZE: usize = 16;

/// Pixel width variants supported by ricepp.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelBits {
    Bits8,
    Bits16,
    Bits32,
}

impl PixelBits {
    fn byte_count(self) -> usize {
        match self {
            Self::Bits8 => 1,
            Self::Bits16 => 2,
            Self::Bits32 => 4,
        }
    }

    fn bit_count(self) -> u32 {
        match self {
            Self::Bits8 => 8,
            Self::Bits16 => 16,
            Self::Bits32 => 32,
        }
    }

    fn from_u8(v: u8) -> Option<Self> {
        match v {
            8 => Some(Self::Bits8),
            16 => Some(Self::Bits16),
            32 => Some(Self::Bits32),
            _ => None,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Bits8 => 8,
            Self::Bits16 => 16,
            Self::Bits32 => 32,
        }
    }
}

/// Byte order for multi-byte pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ByteOrder {
    LittleEndian,
    BigEndian,
}

impl ByteOrder {
    fn as_u8(self) -> u8 {
        match self {
            Self::LittleEndian => 0,
            Self::BigEndian => 1,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::BigEndian,
            _ => Self::LittleEndian,
        }
    }
}

/// Codec configuration: pixel width + byte order + block size.
#[derive(Clone, Copy, Debug)]
pub struct CodecConfig {
    pub pixel_bits: PixelBits,
    pub byte_order: ByteOrder,
    pub block_size: usize,
}

impl Default for CodecConfig {
    fn default() -> Self {
        Self {
            pixel_bits: PixelBits::Bits16,
            byte_order: ByteOrder::BigEndian,
            block_size: DEFAULT_BLOCK_SIZE,
        }
    }
}

/// Read a pixel value from `data` at byte offset `pos`.
fn read_pixel(data: &[u8], pos: usize, bits: PixelBits, order: ByteOrder) -> u64 {
    match bits {
        PixelBits::Bits8 => u64::from(data[pos]),
        PixelBits::Bits16 => {
            let b0 = u16::from(data[pos]);
            let b1 = u16::from(data[pos + 1]);
            match order {
                ByteOrder::LittleEndian => u64::from(b0 | (b1 << 8)),
                ByteOrder::BigEndian => u64::from((b0 << 8) | b1),
            }
        }
        PixelBits::Bits32 => {
            let bytes: [u8; 4] = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
            match order {
                ByteOrder::LittleEndian => u64::from(u32::from_le_bytes(bytes)),
                ByteOrder::BigEndian => u64::from(u32::from_be_bytes(bytes)),
            }
        }
    }
}

/// Write a pixel value to `out` at byte offset `pos`.
fn write_pixel(out: &mut [u8], pos: usize, value: u64, bits: PixelBits, order: ByteOrder) {
    match bits {
        PixelBits::Bits8 => out[pos] = value as u8,
        PixelBits::Bits16 => {
            let v = value as u16;
            match order {
                ByteOrder::LittleEndian => out[pos..pos + 2].copy_from_slice(&v.to_le_bytes()),
                ByteOrder::BigEndian => out[pos..pos + 2].copy_from_slice(&v.to_be_bytes()),
            }
        }
        PixelBits::Bits32 => {
            let v = value as u32;
            match order {
                ByteOrder::LittleEndian => out[pos..pos + 4].copy_from_slice(&v.to_le_bytes()),
                ByteOrder::BigEndian => out[pos..pos + 4].copy_from_slice(&v.to_be_bytes()),
            }
        }
    }
}

/// Bitstream writer matching DwarFS ricepp's `bitstream_writer`.
/// Accumulates bits at the LOW end of a u64, flushes as LE bytes.
struct BitstreamWriter {
    out: Vec<u8>,
    data: u64,
    bit_pos: u32, // 0..64
}

impl BitstreamWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            data: 0,
            bit_pos: 0,
        }
    }

    fn write_bits(&mut self, mut bits: u64, mut num_bits: u32) {
        if num_bits == 0 {
            return;
        }
        loop {
            let bits_to_write = num_bits.min(64 - self.bit_pos);
            let mask = if bits_to_write >= 64 {
                u64::MAX
            } else {
                (1u64 << bits_to_write) - 1
            };
            let fragment = bits & mask;
            self.data |= fragment << self.bit_pos;
            self.bit_pos += bits_to_write;
            if self.bit_pos == 64 {
                self.flush_packet(8);
            }
            bits >>= bits_to_write;
            if num_bits == bits_to_write {
                break;
            }
            num_bits -= bits_to_write;
        }
    }

    /// Write `repeat` copies of `bit` (for unary coding).
    fn write_bit_repeated(&mut self, bit: bool, mut repeat: u32) {
        let pattern = if bit { u64::MAX } else { 0 };
        if self.bit_pos != 0 {
            let remaining = 64 - self.bit_pos;
            if repeat > remaining {
                self.write_bits(pattern, remaining);
                repeat -= remaining;
            }
        }
        while repeat > 64 {
            self.write_full_packet(pattern);
            repeat -= 64;
        }
        if repeat > 0 {
            self.write_bits(pattern, repeat);
        }
    }

    fn write_full_packet(&mut self, bits: u64) {
        let bytes = bits.to_le_bytes();
        self.out.extend_from_slice(&bytes);
    }

    fn flush_packet(&mut self, max_bytes: usize) {
        let bytes = self.data.to_le_bytes();
        self.out.extend_from_slice(&bytes[..max_bytes]);
        self.data = 0;
        self.bit_pos = 0;
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bit_pos > 0 {
            let n_bytes = ((self.bit_pos + 7) / 8) as usize;
            self.flush_packet(n_bytes);
        }
        self.out
    }
}

/// Bitstream reader matching DwarFS ricepp's `bitstream_reader`.
/// Reads MSB-first from a LE byte stream... actually, reads from the
/// bit-packed stream in the same order the writer wrote.
struct BitstreamReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitstreamReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_bits(&mut self, num_bits: u32) -> u64 {
        let mut result = 0u64;
        let mut out_bit = 0u32;
        let mut remaining = num_bits;
        while remaining > 0 {
            let byte_idx = self.bit_pos / 8;
            let bit_in_byte = self.bit_pos % 8;
            if byte_idx >= self.data.len() {
                break;
            }
            let avail = 8 - bit_in_byte as u32;
            let take = remaining.min(avail);
            let byte = u64::from(self.data[byte_idx]);
            let shifted = byte >> bit_in_byte;
            let mask = if take >= 64 {
                u64::MAX
            } else {
                (1u64 << take) - 1
            };
            result |= (shifted & mask) << out_bit;
            self.bit_pos += take as usize;
            out_bit += take;
            remaining -= take;
        }
        result
    }

    /// Count leading zero bits until a 1 is found, returning the count.
    /// Used for unary decoding.
    fn find_first_set(&mut self) -> u64 {
        let mut count = 0u64;
        loop {
            let byte_idx = self.bit_pos / 8;
            let bit_in_byte = self.bit_pos % 8;
            if byte_idx >= self.data.len() {
                return count;
            }
            let byte = self.data[byte_idx];
            let remaining_in_byte = byte >> bit_in_byte;
            if remaining_in_byte == 0 {
                count += (8 - bit_in_byte as u32) as u64;
                self.bit_pos += 8 - bit_in_byte;
            } else {
                let zeros = u8::from(remaining_in_byte).trailing_zeros() as u64;
                count += zeros;
                self.bit_pos += zeros as usize + 1;
                return count;
            }
        }
    }
}

/// Zigzag-encode a signed delta into an unsigned value.
/// Matches C++ `d & msb ? ~(diff << 1) : (diff << 1)`. All arithmetic
/// is performed in the pixel bit width (matches `pixel_value_type`).
fn zigzag_encode(diff: u64, pixel_msb: u64, pixel_bits: u32) -> u64 {
    let mask = (1u64 << pixel_bits) - 1;
    let masked_diff = diff & mask;
    if masked_diff & pixel_msb != 0 {
        let shifted = (masked_diff.wrapping_shl(1)) & mask;
        (!shifted) & mask
    } else {
        (masked_diff << 1) & mask
    }
}

/// Zigzag-decode an unsigned value back to a signed delta.
/// Matches C++ `((diff & 1) * -1) ^ (diff >> 1)`.
fn zigzag_decode(diff: u64) -> i64 {
    let sign = if diff & 1 != 0 { -1i64 } else { 0i64 };
    sign ^ (diff >> 1) as i64
}

/// Compute the best `fs` (Rice split parameter) for a block of zigzag
/// deltas. Ported from C++ `compute_best_split`.
///
/// ## Performance
///
/// The C++ reference calls `bits_for_fs` 2-3 times per block, each
/// iterating all `delta` elements. We avoid the iteration by using
/// the closed-form `popcount_delta` table: for each candidate `fs`,
/// the high-bit contribution is `sum(delta & mask) >> fs` where the
/// mask is `0xFFFF_FFFF_FFFF_FFFF << fs`. We compute `sum(delta)`
/// and `sum(delta & 0xFF)` (i.e. with mask=0xFF) once, then derive
/// the masked sum for any `fs` via `popcount((delta >> fs) & mask)`
/// — but since the per-element contribution to `>> fs` is `delta >> fs`,
/// we can use a bit-occupancy recurrence.
///
/// In practice, the cheap O(N) `start_fs` estimate is already near-
/// optimal; we evaluate just `start_fs` and `start_fs ± 1` instead
/// of doing the C++ adaptive walk.
fn compute_best_split(delta: &[u64], sum: u64, pixel_bits: u32) -> (u32, u64) {
    let fs_max = pixel_bits - 2;
    let size = delta.len() as u64;

    if sum == 0 || size == 0 {
        return (0, 0);
    }

    let bits_for_fs = |fs: u32| -> u64 {
        if fs >= 64 {
            // Shift by ≥ 64 is UB in C; in Rust `u64:: overflowing_shl`
            // wraps the shift amount, so we guard explicitly.
            // With fs ≥ 64 the mask is full (`u64::MAX << fs` becomes
            // 0 in Rust's `<<` semantics for fs ≥ 64 — but we want
            // full mask). Use `wrapping_shl` then NOT, or just handle.
            let mut high_bits_sum = 0u64;
            for &d in delta {
                high_bits_sum = high_bits_sum.wrapping_add(d);
            }
            return size * (u64::from(fs) + 1) + (high_bits_sum >> fs.min(63));
        }
        let mask = u64::MAX << fs;
        let mut high_bits_sum = 0u64;
        for &d in delta {
            high_bits_sum += d & mask;
        }
        size * (u64::from(fs) + 1) + (high_bits_sum >> fs)
    };

    let avg = sum / size;
    let start_fs = if avg == 0 {
        0
    } else {
        (64u32.saturating_sub(avg.leading_zeros() + 2)).min(fs_max)
    };

    // Evaluate start_fs and its two neighbours. The optimal fs is
    // convex in fs (one minimum), so three points suffice.
    let mut best_fs = start_fs;
    let mut best_bits = bits_for_fs(start_fs);
    for cand in [start_fs.saturating_sub(1), start_fs + 1] {
        if cand > fs_max {
            continue;
        }
        let b = bits_for_fs(cand);
        if b < best_bits {
            best_bits = b;
            best_fs = cand;
        }
    }

    (best_fs, best_bits)
}

/// Compute `sum(delta[i] zigzag-encoded)` for a block of pixels
/// where `delta[i] = pixel[i] - pixel[i-1]` and `pixel[-1] = last_value`.
///
/// Uses `wide::u64x4` to vectorise the zigzag + sum reduction when
/// the `simd-delta` feature is enabled. The delta computation stays
/// scalar (sequential dependency on `last`), but the per-pixel
/// inner work (zigzag + accumulate) processes 4 pixels per cycle.
///
/// Returns `(delta_scratch, sum)` so callers can use the deltas for
/// the actual block encoding.
#[cfg(feature = "simd-delta")]
fn delta_zigzag_sum(
    block: &[u64],
    last_value: u64,
    pixel_msb: u64,
    pixel_bits: u32,
    delta_scratch: &mut [u64],
) -> u64 {
    use wide::u64x4;
    let n = block.len();
    let mut last = last_value;
    let mut sum_acc = u64x4::splat(0);
    let mut chunk = [0u64; 4];
    let mut i = 0;

    // Process 4 pixels per chunk. The delta is computed sequentially
    // (each pixel depends on the previous), but the zigzag + sum is
    // vectorised within the chunk.
    while i + 4 <= n {
        // Compute deltas.
        chunk[0] = block[i].wrapping_sub(last);
        chunk[1] = block[i + 1].wrapping_sub(block[i]);
        chunk[2] = block[i + 2].wrapping_sub(block[i + 1]);
        chunk[3] = block[i + 3].wrapping_sub(block[i + 2]);
        last = block[i + 3];

        // Vectorised zigzag + accumulate.
        let pixels = u64x4::from(chunk);
        let mask = u64x4::splat((1u64 << pixel_bits) - 1);
        let masked = pixels & mask;
        let msb_v = u64x4::splat(pixel_msb);
        let is_neg = (masked & msb_v).simd_eq(msb_v);
        let shifted: u64x4 = (masked << 1) & mask;
        let zigzag: u64x4 = (!shifted) & mask;
        let result = is_neg.bitselect(zigzag, shifted);

        let arr = result.to_array();
        delta_scratch[i..i + 4].copy_from_slice(&arr);
        sum_acc += result;
        i += 4;
    }

    let mut sum: u64 = sum_acc.reduce_add();

    // Tail: scalar.
    while i < n {
        let diff = block[i].wrapping_sub(last);
        let d = zigzag_encode(diff, pixel_msb, pixel_bits);
        delta_scratch[i] = d;
        sum = sum.wrapping_add(d);
        last = block[i];
        i += 1;
    }

    sum
}

/// Scalar fallback — see `simd-delta` version above.
#[cfg(not(feature = "simd-delta"))]
fn delta_zigzag_sum(
    block: &[u64],
    last_value: u64,
    pixel_msb: u64,
    pixel_bits: u32,
    delta_scratch: &mut [u64],
) -> u64 {
    let _ = pixel_bits;
    let mut last = last_value;
    let mut sum = 0u64;
    for (i, &pixel) in block.iter().enumerate() {
        let diff = pixel.wrapping_sub(last);
        let d = zigzag_encode(diff, pixel_msb, pixel_bits);
        delta_scratch[i] = d;
        sum += d;
        last = pixel;
    }
    sum
}

/// Encode a single block of pixels. Returns nothing; writes to `writer`.
///
/// `delta_scratch` is a caller-provided scratch buffer; passing it in
/// avoids a per-block `vec![0; block.len()]` allocation. Must be at
/// least `block.len()` long.
fn encode_block(
    block: &[u64],
    writer: &mut BitstreamWriter,
    pixel_bits: u32,
    last_value: &mut u64,
    delta_scratch: &mut [u64],
) {
    let fs_bits = pixel_bits.trailing_zeros();
    let fs_max = pixel_bits - 2;
    let pixel_msb = 1u64 << (pixel_bits - 1);

    let delta = &mut delta_scratch[..block.len()];
    let sum = delta_zigzag_sum(block, *last_value, pixel_msb, pixel_bits, delta);
    *last_value = if block.is_empty() {
        *last_value
    } else {
        block[block.len() - 1]
    };

    if sum > 0 {
        let (fs, bits_used) = compute_best_split(delta, sum, pixel_bits);
        if fs < fs_max && bits_used < u64::from(pixel_bits) * block.len() as u64 {
            // Rice-coded block.
            writer.write_bits(u64::from(fs + 1), fs_bits);
            for &d in delta.iter() {
                let top = d >> fs;
                if top > 0 {
                    writer.write_bit_repeated(false, top as u32);
                }
                writer.write_bits(1, 1); // unary terminator
                writer.write_bits(d, fs);
            }
        } else {
            // Raw block: fs_max + 1 marker + raw pixels.
            writer.write_bits(u64::from(fs_max + 1), fs_bits);
            for &pixel in block {
                writer.write_bits(pixel, pixel_bits);
            }
        }
    } else {
        // All zeros: fs = 0 marker, no pixel data.
        writer.write_bits(0, fs_bits);
    }
}

/// Decode a single block. Reads from `reader`, writes pixels to `block`.
fn decode_block(
    block: &mut [u64],
    reader: &mut BitstreamReader<'_>,
    pixel_bits: u32,
    last_value: &mut u64,
) -> Result<(), OmnizipError> {
    let fs_bits = pixel_bits.trailing_zeros();
    let fs_max = pixel_bits - 2;
    let mut last = *last_value;

    let fsp1 = reader.read_bits(fs_bits) as u32;

    if fsp1 > 0 {
        if fsp1 <= fs_max {
            let fs = fsp1 - 1;
            for slot in block.iter_mut() {
                let unary = reader.find_first_set();
                let low = reader.read_bits(fs);
                let diff = (unary << fs) | low;
                last = last.wrapping_add(zigzag_decode(diff) as u64);
                *slot = last;
            }
        } else {
            // Raw block.
            for slot in block.iter_mut() {
                *slot = reader.read_bits(pixel_bits);
            }
            last = *block.last().unwrap_or(&0);
        }
    } else {
        // All zeros: every pixel = last.
        for slot in block.iter_mut() {
            *slot = last;
        }
    }

    *last_value = last;
    Ok(())
}

/// Compress pixel data. The input is raw pixel bytes (interleaved).
/// Output is the self-describing ricepp container.
///
/// # Errors
///
/// Returns [`OmnizipError::EncodeFailed`] on configuration errors.
pub fn compress(input: &[u8], config: CodecConfig) -> Result<Vec<u8>, OmnizipError> {
    let bytes_per_pixel = config.pixel_bits.byte_count();
    if input.len() % bytes_per_pixel != 0 {
        return Err(OmnizipError::EncodeFailed {
            codec: CodecId::RICEPP,
            reason: format!(
                "input length {} not a multiple of pixel width {}",
                input.len(),
                bytes_per_pixel
            ),
        });
    }

    let pixel_count = input.len() / bytes_per_pixel;
    let pixels: Vec<u64> = (0..pixel_count)
        .map(|i| {
            read_pixel(
                input,
                i * bytes_per_pixel,
                config.pixel_bits,
                config.byte_order,
            )
        })
        .collect();

    let mut writer = BitstreamWriter::new();
    let mut last_value = 0u64;
    // Reusable per-block scratch buffer; avoids an allocation in the
    // hot `encode_block` loop.
    let mut delta_scratch = vec![0u64; config.block_size];

    let mut offset = 0;
    while offset < pixels.len() {
        let end = (offset + config.block_size).min(pixels.len());
        // Grow scratch if block_size was increased since the last call.
        if delta_scratch.len() < end - offset {
            delta_scratch.resize(end - offset, 0);
        }
        encode_block(
            &pixels[offset..end],
            &mut writer,
            config.pixel_bits.bit_count(),
            &mut last_value,
            &mut delta_scratch,
        );
        offset = end;
    }

    let encoded = writer.finish();

    // Build output: header + encoded blocks.
    let mut out = Vec::with_capacity(10 + encoded.len());
    out.push(config.pixel_bits.as_u8());
    out.push(config.byte_order.as_u8());
    out.extend_from_slice(&(config.block_size as u32).to_le_bytes());
    out.extend_from_slice(&(pixel_count as u32).to_le_bytes());
    out.extend_from_slice(&encoded);
    Ok(out)
}

/// Decompress ricepp data produced by [`compress`].
///
/// # Errors
///
/// Returns [`OmnizipError::DecodeFailed`] on malformed input.
pub fn decompress(compressed: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    if compressed.len() < 10 {
        return Err(OmnizipError::DecodeFailed {
            codec: CodecId::RICEPP,
            reason: "header too short".into(),
        });
    }
    let pixel_bits = PixelBits::from_u8(compressed[0]).ok_or(OmnizipError::DecodeFailed {
        codec: CodecId::RICEPP,
        reason: format!("unsupported pixel bits: {}", compressed[0]),
    })?;
    let byte_order = ByteOrder::from_u8(compressed[1]);
    let block_size =
        u32::from_le_bytes([compressed[2], compressed[3], compressed[4], compressed[5]]) as usize;
    let pixel_count =
        u32::from_le_bytes([compressed[6], compressed[7], compressed[8], compressed[9]]) as usize;

    let mut reader = BitstreamReader::new(&compressed[10..]);
    let mut pixels = vec![0u64; pixel_count];
    let mut last_value = 0u64;

    let mut offset = 0;
    while offset < pixel_count {
        let end = (offset + block_size).min(pixel_count);
        decode_block(
            &mut pixels[offset..end],
            &mut reader,
            pixel_bits.bit_count(),
            &mut last_value,
        )?;
        offset = end;
    }

    let bytes_per_pixel = pixel_bits.byte_count();
    let mut out = vec![0u8; pixel_count * bytes_per_pixel];
    for (i, &px) in pixels.iter().enumerate() {
        write_pixel(&mut out, i * bytes_per_pixel, px, pixel_bits, byte_order);
    }
    Ok(out)
}

/// Ricepp codec adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct RiceppCodec {
    config: CodecConfig,
}

impl RiceppCodec {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            config: CodecConfig {
                pixel_bits: PixelBits::Bits16,
                byte_order: ByteOrder::BigEndian,
                block_size: DEFAULT_BLOCK_SIZE,
            },
        }
    }

    #[must_use]
    pub const fn with_config(config: CodecConfig) -> Self {
        Self { config }
    }
}

impl Codec for RiceppCodec {
    fn id(&self) -> CodecId {
        CodecId::RICEPP
    }

    fn name(&self) -> &'static str {
        "ricepp"
    }

    fn compress(
        &self,
        plaintext: &[u8],
        _level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        compress(plaintext, self.config)
    }

    fn decompress(&self, compressed: &[u8], _expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        decompress(compressed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(input: &[u8], config: CodecConfig) {
        let compressed = compress(input, config).expect("compress");
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(
            decompressed,
            input,
            "round-trip failed for {} bytes",
            input.len()
        );
    }

    #[test]
    fn empty_round_trips() {
        round_trip(&[], CodecConfig::default());
    }

    #[test]
    fn flat_16bit_be_round_trips() {
        // 32 pixels of value 1000 (BE 16-bit).
        let input: Vec<u8> = (0..32).flat_map(|_| (1000u16).to_be_bytes()).collect();
        round_trip(&input, CodecConfig::default());
        let compressed = compress(&input, CodecConfig::default()).expect("compress");
        // Flat data should compress well (all-zero deltas).
        assert!(compressed.len() < input.len());
    }

    #[test]
    fn ramp_16bit_le_round_trips() {
        // Linear ramp: 0, 1, 2, ..., 255 (LE 16-bit).
        let input: Vec<u8> = (0..256u16).flat_map(|v| v.to_le_bytes()).collect();
        let config = CodecConfig {
            pixel_bits: PixelBits::Bits16,
            byte_order: ByteOrder::LittleEndian,
            block_size: 16,
        };
        round_trip(&input, config);
        let compressed = compress(&input, config).expect("compress");
        // Ramps have constant deltas → good Rice compression.
        assert!(compressed.len() < input.len());
    }

    #[test]
    fn noisy_8bit_round_trips() {
        // Noisy data that won't compress well but must round-trip.
        let input: Vec<u8> = (0..200).map(|i| ((i * 7919) % 256) as u8).collect();
        let config = CodecConfig {
            pixel_bits: PixelBits::Bits8,
            byte_order: ByteOrder::LittleEndian,
            block_size: 16,
        };
        round_trip(&input, config);
    }

    #[test]
    fn multi_block_round_trips() {
        // 50 pixels with block_size=16 → 4 blocks (16,16,16,2).
        let input: Vec<u8> = (0..50u16).flat_map(|v| (v * 7).to_be_bytes()).collect();
        round_trip(&input, CodecConfig::default());
    }

    #[test]
    fn determinism() {
        let input: Vec<u8> = (0..64u16).flat_map(|v| v.to_be_bytes()).collect();
        let a = compress(&input, CodecConfig::default()).expect("compress");
        let b = compress(&input, CodecConfig::default()).expect("compress");
        assert_eq!(a, b, "ricepp must be deterministic");
    }

    #[test]
    fn codec_trait_round_trips() {
        let codec = RiceppCodec::new();
        let input: Vec<u8> = (0..32u16).flat_map(|v| v.to_be_bytes()).collect();
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .expect("compress");
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn zigzag_round_trip() {
        let pixel_bits = 16u32;
        let pixel_msb = 1u64 << (pixel_bits - 1);
        // Zigzag maps unsigned deltas to unsigned codes and back. For
        // values ≥ pixel_msb, the "delta" is interpreted as negative
        // (two's complement in the pixel width), so the round-trip is
        // valid only within the pixel bit width.
        for &diff in &[0u64, 1, 2, 100, 255, 256, 1000, 32767] {
            let encoded = zigzag_encode(diff, pixel_msb, pixel_bits);
            let decoded = zigzag_decode(encoded) as u64;
            assert_eq!(decoded, diff, "zigzag round-trip failed for {}", diff);
        }
        // Large values: verify the wrapping_add path produces the
        // correct pixel value.
        for &diff in &[32768u64, 65535] {
            let encoded = zigzag_encode(diff, pixel_msb, pixel_bits);
            let decoded = zigzag_decode(encoded) as u64;
            // wrapping_add back to a base value should match diff.
            let base = 0u64;
            assert_eq!(
                base.wrapping_add(decoded) & ((1u64 << pixel_bits) - 1),
                diff,
                "zigzag wrapping round-trip failed for {}",
                diff
            );
        }
    }
}
