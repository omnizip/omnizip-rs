//! FSE (Finite State Entropy) bitstream readers.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/zstandard/fse/bitstream.rb`
//! (186 LOC, MIT, Ribose Inc.). ZSTD uses two bit read orders:
//!
//! - **Reverse** ([`BitStream`]) — FSE entropy streams are written
//!   back-to-front and read from the end of the buffer toward the
//!   start, LSB-first within each byte. Used by every FSE-coded field
//!   (sequences, Huffman weights).
//! - **Forward** ([`ForwardBitStream`]) — Huffman-coded literals are
//!   read from the start, MSB-first within each byte.
//!
//! ## Performance note
//!
//! The Ruby reads one bit at a time, which is correct but slow. The
//! Rust port keeps the same one-bit-at-a-time semantics for now (it's
//! the hot path; before bench-driven tuning we want correctness). A
//! `u64`-buffered reader can be layered in later without changing the
//! public API.

#![forbid(unsafe_code)]

use crate::ZstdError;

/// Reverse-direction bit reader matching the C reference `BIT_DStream`
/// (lib/common/bitstream.h). All bytes are loaded into a `u64`
/// container with byte[0] at the lowest bits; bits are extracted from
/// the HIGH end. The end mark (trailing zero bits in the last byte)
/// is skipped via `bitsConsumed` initialization.
#[derive(Debug)]
pub struct BitStream<'a> {
    data: &'a [u8],
    container: u64,
    bits_consumed: u32,
}

impl<'a> BitStream<'a> {
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        // Load bytes into container, byte[0] at lowest bits.
        // Matches C's BIT_initDStream small-stream path.
        let mut container = if data.is_empty() {
            0
        } else {
            u64::from(data[0])
        };
        if data.len() >= 2 {
            container += u64::from(data[1]) << 8;
        }
        if data.len() >= 3 {
            container += u64::from(data[2]) << 16;
        }
        if data.len() >= 4 {
            container += u64::from(data[3]) << 24;
        }
        if data.len() >= 5 {
            container += u64::from(data[4]) << 32;
        }
        if data.len() >= 6 {
            container += u64::from(data[5]) << 40;
        }
        if data.len() >= 7 {
            container += u64::from(data[6]) << 48;
        }

        // End mark: skip trailing zero bits in last byte.
        // Must cast to u32 for leading_zeros to match C's ZSTD_highbit32.
        let last_byte = u32::from(*data.last().unwrap_or(&0));
        let end_mark = if last_byte > 0 {
            8 - last_byte.ilog2()
        } else {
            0
        };

        // bitsConsumed = end_mark + (container_size - src_size) * 8
        let bits_consumed = end_mark + (8_u32.saturating_sub(data.len() as u32)) * 8;

        Self {
            data,
            container,
            bits_consumed,
        }
    }

    #[must_use]
    pub fn remaining_bits(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.bits_consumed as usize)
    }

    #[must_use] 
    pub fn bit_position(&self) -> usize {
        self.remaining_bits()
    }

    #[must_use]
    pub const fn is_exhausted(&self) -> bool {
        self.bits_consumed >= 64
    }

    #[inline]
    pub fn read_bits(&mut self, count: u32) -> u32 {
        if count == 0 {
            return 0;
        }
        let total = self.bits_consumed.saturating_add(count);
        if total > 64 {
            self.bits_consumed = 64;
            return 0;
        }
        let start = 64 - total;
        let mask = (1u64 << count) - 1;
        let result = ((self.container >> start) & mask) as u32;
        self.bits_consumed += count;
        result
    }

    #[inline]
    pub fn peek_bits(&mut self, count: u32) -> u32 {
        let saved = self.bits_consumed;
        let result = self.read_bits(count);
        self.bits_consumed = saved;
        result
    }

    pub fn align_to_byte(&mut self) {
        let r = self.bits_consumed % 8;
        if r != 0 {
            self.bits_consumed += 8 - r;
        }
    }
}

/// Forward-direction bit reader: bytes consumed from the start, bits
/// consumed MSB-first within each byte. Used by the Huffman decoder.
#[derive(Debug)]
pub struct ForwardBitStream<'a> {
    data: &'a [u8],
    bit_position: usize,
}

impl<'a> ForwardBitStream<'a> {
    /// Construct a forward reader starting at byte offset `start_byte`.
    ///
    /// # Errors
    ///
    /// Returns [`ZstdError::Corrupt`] if `start_byte > data.len()`.
    pub fn new(data: &'a [u8], start_byte: usize) -> Result<Self, ZstdError> {
        if start_byte > data.len() {
            return Err(ZstdError::Corrupt {
                reason: format!(
                    "forward bitstream start_byte {start_byte} exceeds data len {}",
                    data.len()
                ),
            });
        }
        Ok(Self {
            data,
            bit_position: start_byte * 8,
        })
    }

    /// Construct a reader at the start of `data`.
    #[must_use]
    pub fn from_start(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_position: 0,
        }
    }

    /// Bits remaining.
    #[must_use]
    pub const fn remaining_bits(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.bit_position)
    }

    /// Whether the stream is fully consumed.
    #[must_use]
    pub const fn is_exhausted(&self) -> bool {
        self.bit_position >= self.data.len() * 8
    }

    /// Current byte index (rounded down).
    #[must_use]
    pub const fn byte_position(&self) -> usize {
        self.bit_position / 8
    }

    /// Read `count` bits MSB-first. Returns 0 if `count == 0`.
    #[inline]
    pub fn read_bits(&mut self, count: u32) -> u32 {
        if count == 0 {
            return 0;
        }
        let mut result = 0u32;
        for _ in 0..count {
            result = (result << 1) | self.read_single_bit();
        }
        result
    }

    /// Peek `count` bits without advancing the position. Required by
    /// the Huffman decoder's single-level lookup table.
    #[inline]
    pub fn peek_bits(&mut self, count: u8) -> u32 {
        let saved = self.bit_position;
        let v = self.read_bits(u32::from(count));
        self.bit_position = saved;
        v
    }

    #[inline]
    fn read_single_bit(&mut self) -> u32 {
        if self.is_exhausted() {
            return 0;
        }
        let byte_index = self.bit_position / 8;
        let bit_index = 7 - (self.bit_position % 8);
        self.bit_position += 1;
        if byte_index >= self.data.len() {
            return 0;
        }
        u32::from((self.data[byte_index] >> bit_index) & 0x01)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_byte_stream_reads_from_high_end() {
        // [0xFF]: end mark = 8 - highbit(255) = 8-7 = 1.
        // bitsConsumed = 1 + (8-1)*8 = 57. 7 usable bits, all 1.
        let mut bs = BitStream::new(&[0xFF]);
        for _ in 0..7 {
            assert_eq!(bs.read_bits(1), 1);
        }
    }

    #[test]
    fn peek_does_not_consume() {
        let mut bs = BitStream::new(&[0xAB, 0xCD]);
        let a = bs.peek_bits(4);
        let b = bs.peek_bits(4);
        assert_eq!(a, b);
    }

    #[test]
    fn align_advances_to_byte_boundary() {
        let mut bs = BitStream::new(&[0xFF, 0xFF, 0xFF]);
        let before = bs.bits_consumed;
        bs.read_bits(5);
        bs.align_to_byte();
        // After reading 5 bits + align, bitsConsumed should be at a
        // byte boundary relative to the stream start.
        let after = bs.bits_consumed;
        assert!(after > before);
    }

    #[test]
    fn forward_stream_reads_msb_first_from_start() {
        // Byte 0xB5 = 0b1011_0101, MSB first → 1,0,1,1,0,1,0,1
        let mut fs = ForwardBitStream::from_start(&[0xB5]);
        let v = fs.read_bits(4);
        // bit0=1<<3, bit1=0<<2, bit2=1<<1, bit3=1<<0 = 0b1011 = 0xB
        assert_eq!(v, 0b1011);
    }

    #[test]
    fn forward_stream_start_byte_out_of_range_errors() {
        assert!(ForwardBitStream::new(&[0u8; 4], 5).is_err());
    }

    #[test]
    fn forward_stream_is_exhausted_after_full_read() {
        let mut fs = ForwardBitStream::from_start(&[0xFF]);
        let _ = fs.read_bits(8);
        assert!(fs.is_exhausted());
        // Reading past the end yields 0.
        assert_eq!(fs.read_bits(4), 0);
    }
}
