//! Shared bit-level I/O primitives for codec implementations.
//!
//! All entropy coders (FLAC, Brotli, DEFLATE, ZSTD FSE, GLZA) read and
//! write individual bits from/to byte streams. The two common bit
//! orderings are:
//!
//! - **MSB-first** (big-endian bit order): the most-significant bit of
//!   each byte is consumed first. Used by FLAC, Brotli, DEFLATE.
//! - **LSB-first** (little-endian bit order): the least-significant bit
//!   is consumed first. Used by ZSTD FSE.
//!
//! This module provides both variants with a common API, eliminating
//! ~400 LOC of duplicated `BitReader` / `BitWriter` implementations
//! across the workspace.
//!
//! ## Design
//!
//! Each reader/writer is parameterised by bit order (via separate types,
//! not runtime flags — the compiler specialises and inlines). The API
//! is deliberately minimal:
//!
//! ```ignore
//! // Reading
//! let mut br = BitReaderBE::new(input);
//! let bit: bool = br.read_bit()?;
//! let val: u32 = br.read_bits(5)?;
//!
//! // Writing
//! let mut bw = BitWriterBE::new();
//! bw.write_bit(true);
//! bw.write_bits(0x1F, 5);
//! let bytes: Vec<u8> = bw.finish();
//! ```
//!
//! ## Performance
//!
//! `read_bits` / `write_bits` accumulate bits in a `u32` accumulator,
//! refilling from the byte stream when the accumulator runs low. This
//! avoids per-bit byte access and lets the optimiser vectorise.
//!
//! ## Determinism
//!
//! No internal state beyond the accumulator and byte cursor. Same input
//! → same output, always.

#![forbid(unsafe_code)]

use crate::OmnizipError;

// ── MSB-first (big-endian bit order) ──────────────────────────────────────

/// Big-endian bit reader. Consumes bits from the most-significant bit
/// of each byte first. Used by FLAC, Brotli, DEFLATE.
#[derive(Debug)]
pub struct BitReaderBE<'a> {
    data: &'a [u8],
    pos: usize,
    /// Bit accumulator. Bits are stored MSB-aligned: the next bit to
    /// read is at position `bit_count - 1`.
    acc: u64,
    /// Number of valid bits currently in `acc`.
    bit_count: u32,
}

impl<'a> BitReaderBE<'a> {
    /// Construct a reader over `data`.
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            acc: 0,
            bit_count: 0,
        }
    }

    /// Refill the accumulator to at least 56 bits (7 bytes) if possible.
    #[inline]
    fn refill(&mut self) -> Result<(), OmnizipError> {
        while self.bit_count <= 56 {
            if self.pos < self.data.len() {
                self.acc |= u64::from(self.data[self.pos]) << (56 - self.bit_count);
                self.pos += 1;
                self.bit_count += 8;
            } else {
                break;
            }
        }
        Ok(())
    }

    /// Read a single bit. Returns `true` for 1, `false` for 0.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::Corrupt`] if the stream is exhausted.
    #[inline]
    pub fn read_bit(&mut self) -> Result<bool, OmnizipError> {
        if self.bit_count == 0 {
            self.refill()?;
        }
        if self.bit_count == 0 {
            return Err(OmnizipError::Corrupt {
                codec: crate::CodecId::LZMA,
                reason: "bit stream exhausted".into(),
            });
        }
        self.bit_count -= 1;
        let bit = (self.acc >> 63) & 1 == 1;
        self.acc <<= 1;
        Ok(bit)
    }

    /// Read `nbits` bits as a `u32`. The first bit read is the MSB of
    /// the result.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::Corrupt`] if the stream is exhausted.
    #[inline]
    pub fn read_bits(&mut self, nbits: u32) -> Result<u32, OmnizipError> {
        debug_assert!(nbits <= 32);
        if nbits == 0 {
            return Ok(0);
        }
        while self.bit_count < nbits {
            self.refill()?;
            if self.bit_count < nbits && self.pos >= self.data.len() {
                break;
            }
        }
        if self.bit_count < nbits {
            return Err(OmnizipError::Corrupt {
                codec: crate::CodecId::LZMA,
                reason: format!(
                    "bit stream exhausted: needed {nbits} bits, have {}",
                    self.bit_count
                ),
            });
        }
        self.bit_count -= nbits;
        let result = (self.acc >> (64 - nbits)) as u32;
        self.acc <<= nbits;
        Ok(result)
    }

    /// Current byte position in the input (for diagnostics).
    #[must_use]
    pub const fn position(&self) -> usize {
        self.pos
    }

    /// Align to the next byte boundary. Discards remaining bits in the
    /// current byte.
    pub fn align_to_byte(&mut self) {
        let waste = self.bit_count & 7;
        self.bit_count -= waste;
        self.acc <<= waste;
    }

    /// Read raw bytes directly (bypassing bit-level access). The reader
    /// must be byte-aligned (see [`align_to_byte`](Self::align_to_byte)).
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::Corrupt`] if not aligned or stream is
    /// exhausted.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], OmnizipError> {
        if self.bit_count & 7 != 0 {
            return Err(OmnizipError::Corrupt {
                codec: crate::CodecId::LZMA,
                reason: "read_bytes called when not byte-aligned".into(),
            });
        }
        // Flush the accumulator's whole bytes back into the position.
        let buffered_bytes = (self.bit_count / 8) as usize;
        let start = self.pos.saturating_sub(buffered_bytes);
        if start + n > self.data.len() {
            return Err(OmnizipError::Corrupt {
                codec: crate::CodecId::LZMA,
                reason: format!("read_bytes needs {n}, have {}", self.data.len() - start),
            });
        }
        let result = &self.data[start..start + n];
        // Advance past the returned bytes.
        self.pos = start + n;
        self.acc = 0;
        self.bit_count = 0;
        Ok(result)
    }
}

/// Big-endian bit writer. Emits bits MSB-first into a growing `Vec<u8>`.
#[derive(Debug, Default)]
pub struct BitWriterBE {
    out: Vec<u8>,
    acc: u64,
    bit_count: u32,
}

impl BitWriterBE {
    /// Construct an empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Write a single bit.
    #[inline]
    pub fn write_bit(&mut self, bit: bool) {
        self.write_bits(u32::from(bit), 1);
    }

    /// Write the low `nbits` of `value`, MSB first.
    #[inline]
    pub fn write_bits(&mut self, value: u32, nbits: u32) {
        debug_assert!(nbits <= 32);
        if nbits == 0 {
            return;
        }
        let v = if nbits < 32 {
            u64::from(value & ((1u32 << nbits) - 1))
        } else {
            u64::from(value)
        };
        // Place the new bits below the existing ones.
        self.acc |= v << (64 - self.bit_count - nbits);
        self.bit_count += nbits;
        // Flush whole bytes.
        while self.bit_count >= 8 {
            self.out.push((self.acc >> 56) as u8);
            self.acc <<= 8;
            self.bit_count -= 8;
        }
    }

    /// Pad the final byte with zeros and return the written bytes.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        if self.bit_count > 0 {
            // Flush remaining bits, zero-padded.
            self.out.push((self.acc >> 56) as u8);
        }
        self.out
    }

    /// Number of bytes written so far (excluding partial byte in accumulator).
    #[must_use]
    pub fn len(&self) -> usize {
        self.out.len()
    }

    /// Whether no bits have been written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.out.is_empty() && self.bit_count == 0
    }
}

// ── LSB-first (little-endian bit order) ───────────────────────────────────

/// Little-endian bit reader. Consumes bits from the least-significant
/// bit of each byte first. Used by ZSTD FSE.
#[derive(Debug)]
pub struct BitReaderLE<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    bit_count: u32,
}

impl<'a> BitReaderLE<'a> {
    /// Construct a reader over `data`.
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            acc: 0,
            bit_count: 0,
        }
    }

    /// Refill the accumulator to at least 56 bits.
    #[inline]
    fn refill(&mut self) -> Result<(), OmnizipError> {
        while self.bit_count <= 56 {
            if self.pos < self.data.len() {
                self.acc |= u64::from(self.data[self.pos]) << self.bit_count;
                self.pos += 1;
                self.bit_count += 8;
            } else {
                break;
            }
        }
        Ok(())
    }

    /// Read a single bit.
    #[inline]
    pub fn read_bit(&mut self) -> Result<bool, OmnizipError> {
        if self.bit_count == 0 {
            self.refill()?;
        }
        if self.bit_count == 0 {
            return Err(OmnizipError::Corrupt {
                codec: crate::CodecId::LZMA,
                reason: "bit stream exhausted".into(),
            });
        }
        self.bit_count -= 1;
        let bit = self.acc & 1 == 1;
        self.acc >>= 1;
        Ok(bit)
    }

    /// Read `nbits` bits. The first bit read is the LSB of the result.
    #[inline]
    pub fn read_bits(&mut self, nbits: u32) -> Result<u32, OmnizipError> {
        debug_assert!(nbits <= 32);
        if nbits == 0 {
            return Ok(0);
        }
        while self.bit_count < nbits {
            self.refill()?;
            if self.bit_count < nbits && self.pos >= self.data.len() {
                break;
            }
        }
        if self.bit_count < nbits {
            return Err(OmnizipError::Corrupt {
                codec: crate::CodecId::LZMA,
                reason: format!(
                    "bit stream exhausted: needed {nbits} bits, have {}",
                    self.bit_count
                ),
            });
        }
        let mask = (1u64 << nbits) - 1;
        let result = (self.acc & mask) as u32;
        self.acc >>= nbits;
        self.bit_count -= nbits;
        Ok(result)
    }

    /// Current byte position.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.pos
    }
}

/// Little-endian bit writer. Emits bits LSB-first.
#[derive(Debug, Default)]
pub struct BitWriterLE {
    out: Vec<u8>,
    acc: u64,
    bit_count: u32,
}

impl BitWriterLE {
    /// Construct an empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Write a single bit.
    #[inline]
    pub fn write_bit(&mut self, bit: bool) {
        self.write_bits(u32::from(bit), 1);
    }

    /// Write the low `nbits` of `value`, LSB first.
    #[inline]
    pub fn write_bits(&mut self, value: u32, nbits: u32) {
        debug_assert!(nbits <= 32);
        if nbits == 0 {
            return;
        }
        let v = if nbits < 32 {
            u64::from(value & ((1u32 << nbits) - 1))
        } else {
            u64::from(value)
        };
        self.acc |= v << self.bit_count;
        self.bit_count += nbits;
        while self.bit_count >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.bit_count -= 8;
        }
    }

    /// Pad the final byte with zeros and return the written bytes.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        if self.bit_count > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }

    /// Number of bytes written so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.out.len()
    }

    /// Whether no bits have been written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.out.is_empty() && self.bit_count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── BitReaderBE tests ──────────────────────────────────────────────

    #[test]
    fn be_read_single_bytes() {
        // 0b10101010 = 0xAA
        let mut br = BitReaderBE::new(&[0xAA]);
        assert_eq!(br.read_bit().unwrap(), true);
        assert_eq!(br.read_bit().unwrap(), false);
        assert_eq!(br.read_bit().unwrap(), true);
        assert_eq!(br.read_bit().unwrap(), false);
        assert_eq!(br.read_bit().unwrap(), true);
        assert_eq!(br.read_bit().unwrap(), false);
        assert_eq!(br.read_bit().unwrap(), true);
        assert_eq!(br.read_bit().unwrap(), false);
    }

    #[test]
    fn be_read_bits_multi_byte() {
        // 0b10000000 0b00000000 = 0x8000. First 9 bits: 1,00000000
        let mut br = BitReaderBE::new(&[0x80, 0x00]);
        assert_eq!(br.read_bits(1).unwrap(), 1);
        assert_eq!(br.read_bits(8).unwrap(), 0);
        assert_eq!(br.read_bits(7).unwrap(), 0);
    }

    #[test]
    fn be_read_bits_12() {
        // 0xAB, 0xCD → bits: 1010_1011_1100_1101
        // read 12: 1010_1011_1100 = 0xABC
        let mut br = BitReaderBE::new(&[0xAB, 0xCD]);
        assert_eq!(br.read_bits(12).unwrap(), 0xABC);
    }

    #[test]
    fn be_eof_returns_error() {
        let mut br = BitReaderBE::new(&[0xFF]);
        br.read_bits(8).unwrap();
        assert!(br.read_bit().is_err());
    }

    #[test]
    fn be_align_to_byte() {
        // Read 3 bits, align, then read bytes.
        let mut br = BitReaderBE::new(&[0b1110_0001, 0x42, 0x43]);
        assert_eq!(br.read_bits(3).unwrap(), 0b111);
        br.align_to_byte();
        let bytes = br.read_bytes(2).unwrap();
        assert_eq!(bytes, &[0x42, 0x43]);
    }

    // ── BitWriterBE tests ──────────────────────────────────────────────

    #[test]
    fn be_write_bits_round_trips() {
        let mut bw = BitWriterBE::new();
        bw.write_bits(0xABC, 12);
        bw.write_bits(0, 4);
        let bytes = bw.finish();
        assert_eq!(bytes, &[0xAB, 0xC0]);
    }

    #[test]
    fn be_write_bit_by_bit() {
        let mut bw = BitWriterBE::new();
        for bit in [true, false, true, false, true, false, true, false] {
            bw.write_bit(bit);
        }
        let bytes = bw.finish();
        assert_eq!(bytes, &[0xAA]);
    }

    #[test]
    fn be_writer_reader_round_trip() {
        let mut bw = BitWriterBE::new();
        bw.write_bits(0x1F, 5);
        bw.write_bits(0x00, 3);
        bw.write_bits(0x42, 8);
        let bytes = bw.finish();

        let mut br = BitReaderBE::new(&bytes);
        assert_eq!(br.read_bits(5).unwrap(), 0x1F);
        assert_eq!(br.read_bits(3).unwrap(), 0);
        assert_eq!(br.read_bits(8).unwrap(), 0x42);
    }

    // ── BitReaderLE tests ──────────────────────────────────────────────

    #[test]
    fn le_read_single_byte() {
        // 0b00000001 = 0x01, LSB first: 1,0,0,0,0,0,0,0
        let mut br = BitReaderLE::new(&[0x01]);
        assert_eq!(br.read_bit().unwrap(), true);
        assert_eq!(br.read_bit().unwrap(), false);
    }

    #[test]
    fn le_read_bits_12() {
        // 0xCD, 0xAB → little-endian: 0xABCD
        // LSB first: first 4 bits = 0xD (low nibble of 0xCD)
        let mut br = BitReaderLE::new(&[0xCD, 0xAB]);
        assert_eq!(br.read_bits(4).unwrap(), 0xD);
        assert_eq!(br.read_bits(4).unwrap(), 0xC);
        assert_eq!(br.read_bits(4).unwrap(), 0xB);
        assert_eq!(br.read_bits(4).unwrap(), 0xA);
    }

    #[test]
    fn le_eof_returns_error() {
        let mut br = BitReaderLE::new(&[0xFF]);
        br.read_bits(8).unwrap();
        assert!(br.read_bit().is_err());
    }

    // ── BitWriterLE tests ──────────────────────────────────────────────

    #[test]
    fn le_write_bits_round_trips() {
        let mut bw = BitWriterLE::new();
        bw.write_bits(0x0D, 4);
        bw.write_bits(0x0C, 4);
        bw.write_bits(0x0B, 4);
        bw.write_bits(0x0A, 4);
        let bytes = bw.finish();
        assert_eq!(bytes, &[0xCD, 0xAB]);
    }

    #[test]
    fn le_writer_reader_round_trip() {
        let mut bw = BitWriterLE::new();
        bw.write_bits(0x42, 8);
        bw.write_bits(0x1F, 5);
        bw.write_bits(0, 3);
        let bytes = bw.finish();

        let mut br = BitReaderLE::new(&bytes);
        assert_eq!(br.read_bits(8).unwrap(), 0x42);
        assert_eq!(br.read_bits(5).unwrap(), 0x1F);
        assert_eq!(br.read_bits(3).unwrap(), 0);
    }

    // ── Determinism ─────────────────────────────────────────────────────

    #[test]
    fn determinism_same_input_same_output() {
        let write = || {
            let mut bw = BitWriterBE::new();
            for i in 0..100u32 {
                bw.write_bits(i, 7);
            }
            bw.finish()
        };
        assert_eq!(write(), write());
    }
}
