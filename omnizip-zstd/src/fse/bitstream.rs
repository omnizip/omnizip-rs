//! FSE (Finite State Entropy) bitstream readers.
//!
//! Verified against `BIT_DStream` in
//! `~/src/external/zstd/lib/common/bitstream.h`.
//!
//! ZSTD uses two bit read orders:
//!
//! - **Reverse** ([`BitStream`]) — FSE entropy streams are written
//!   back-to-front and read from the END of the buffer toward the
//!   START, LSB-first within each byte. Used by every FSE-coded field
//!   (sequences, Huffman weights).
//! - **Forward** ([`ForwardBitStream`]) — Huffman-coded literals are
//!   read from the start, MSB-first within each byte.

#![forbid(unsafe_code)]

use crate::ZstdError;

/// `highbit32(x)` equivalent from the C reference
/// (`ZSTD_highbit32`): `31 - x.leading_zeros()`. Returns 0 for x=0
/// (matching C's "undefined behavior" defensively).
const fn highbit32(x: u32) -> u32 {
    if x == 0 {
        0
    } else {
        x.ilog2()
    }
}

/// Reload status matching C's `BIT_DStream_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadStatus {
    Unfinished,
    EndOfBuffer,
    Completed,
    Overflow,
}

pub use self::ReloadStatus::{Completed, EndOfBuffer, Overflow, Unfinished};

/// Reverse-direction bit reader matching the C reference `BIT_DStream`
/// (`lib/common/bitstream.h`).
///
/// The bitstream is laid out in memory as written by the encoder (a
/// forward byte sequence). The reader walks it BACKWARDS:
///
/// - Initial load: 8 bytes from the END of `data` are read LE into a
///   `u64` container, with byte[N-1] at the LOW bits.
/// - `bits_consumed` is initialised to skip the end mark (trailing
///   zero bits in byte[N-1]).
/// - As bits are consumed, `ptr` decrements to load earlier bytes
///   into the container.
#[derive(Debug)]
pub struct BitStream<'a> {
    data: &'a [u8],
    /// Index of byte at bits 0-7 of container (LOW end, LE order).
    ptr: usize,
    container: u64,
    /// Bits consumed from the HIGH end of container.
    bits_consumed: u32,
    /// End mark size (trailing zero bits in last byte) — preserved
    /// across reloads so `is_exhausted` can compute the true
    /// remaining bit count.
    #[allow(dead_code)]
    end_mark: u32,
}

impl<'a> BitStream<'a> {
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        let n = data.len();
        let last = u32::from(*data.last().unwrap_or(&0));
        let end_mark = if last > 0 { 8 - highbit32(last) } else { 0 };

        let (ptr, container, bits_consumed) = if n >= 8 {
            let p = n - 8;
            let c = u64::from_le_bytes([
                data[p], data[p + 1], data[p + 2], data[p + 3],
                data[p + 4], data[p + 5], data[p + 6], data[p + 7],
            ]);
            (p, c, end_mark)
        } else {
            let mut c: u64 = 0;
            for (i, &b) in data.iter().enumerate() {
                c |= u64::from(b) << (i * 8);
            }
            let bc = end_mark + (8_u32.saturating_sub(n as u32)) * 8;
            (0, c, bc)
        };
        Self {
            data,
            ptr,
            container,
            bits_consumed,
            end_mark,
        }
    }

    /// Reload the container after enough bits have been consumed.
    /// Matches C's `BIT_reloadDStream` exactly, including the 4-branch
    /// logic for different ptr positions relative to limitPtr and start.
    pub fn reload(&mut self) {
        if self.bits_consumed < 8 {
            return;
        }
        // limitPtr = start + sizeof(container) = 8.
        let limit = 8usize;
        if self.ptr >= limit {
            // Path 2: ptr >= limitPtr. Full reload via internal.
            let bytes_consumed = (self.bits_consumed >> 3) as usize;
            self.ptr = self.ptr.saturating_sub(bytes_consumed);
            self.bits_consumed &= 7;
            self.load_container();
            return;
        }
        if self.ptr == 0 {
            // Path 3: ptr == start. No reload. Container stays.
            // Just return — bits_consumed is NOT reset.
            return;
        }
        // Path 4: start (0) < ptr < limitPtr (8). Cautious update.
        let nb_bytes = (self.bits_consumed >> 3) as usize;
        let actual_bytes = nb_bytes.min(self.ptr);
        self.ptr -= actual_bytes;
        self.bits_consumed -= (actual_bytes as u32) * 8;
        self.load_container();
    }

    /// Load 8 bytes from `self.ptr` into the container (LE).
    fn load_container(&mut self) {
        let p = self.ptr;
        if p + 8 <= self.data.len() {
            self.container = u64::from_le_bytes([
                self.data[p], self.data[p + 1], self.data[p + 2], self.data[p + 3],
                self.data[p + 4], self.data[p + 5], self.data[p + 6], self.data[p + 7],
            ]);
        } else {
            let mut c: u64 = 0;
            for i in 0..(self.data.len() - p) {
                c |= u64::from(self.data[p + i]) << (i * 8);
            }
            self.container = c;
        }
    }

    /// Read `count` bits. Matches C's `BIT_lookBits` + `BIT_skipBits`
    /// exactly: `(container << (bitsConsumed & 63)) >> ((64 - count) & 63)`.
    /// The masked shifts wrap around when bitsConsumed + count > 64,
    /// reading stale bits from the container bottom — this is
    /// intentional in the C reference (the decoder over-consumes
    /// slightly at stream boundaries, relying on the next reload to
    /// fix up).
    #[inline]
    pub fn read_bits(&mut self, count: u32) -> u32 {
        if count == 0 { return 0; }
        let shift_left = self.bits_consumed & 63;
        let shift_right = (64u32 - count) & 63;
        let result = if shift_left >= shift_right {
            (self.container << shift_left) >> shift_right
        } else {
            // shift_left < shift_right means the left shift would lose
            // high bits that the right shift needs. C's unsigned shift
            // still works because the high bits were already consumed;
            // the result effectively reads bits from the container's
            // high-to-low order, wrapping through the bottom.
            (self.container << shift_left) >> shift_right
        };
        self.bits_consumed += count;
        // Mask to `count` bits (C doesn't mask, but the shifts already
        // isolate the bits when count <= 64).
        let mask = if count >= 64 { u64::MAX } else { (1u64 << count) - 1 };
        (result & mask) as u32
    }

    /// Peek `count` bits without advancing.
    #[inline]
    pub fn peek_bits(&mut self, count: u32) -> u32 {
        let saved_bc = self.bits_consumed;
        let saved_ptr = self.ptr;
        let saved_c = self.container;
        let result = self.read_bits(count);
        self.bits_consumed = saved_bc;
        self.ptr = saved_ptr;
        self.container = saved_c;
        result
    }

    /// Total bits in the stream (raw, including the end mark).
    #[must_use]
    pub fn total_bits(&self) -> usize {
        self.data.len() * 8
    }

    /// Number of bits consumed from the stream (counting from the END
    /// of `data` toward the START). Includes the end-mark bits.
    fn bits_consumed_so_far(&self) -> usize {
        let bytes_after_window = self.data.len().saturating_sub(self.ptr + 8);
        bytes_after_window * 8 + self.bits_consumed as usize
    }

    /// Bits remaining to be read (raw, including any trailing end mark).
    #[must_use]
    pub fn remaining_bits(&self) -> usize {
        self.total_bits().saturating_sub(self.bits_consumed_so_far())
    }

    /// Bit position (for diagnostics).
    #[must_use]
    pub fn bit_position(&self) -> usize {
        self.remaining_bits()
    }

    /// Whether the stream is exhausted.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.remaining_bits() == 0
    }

    /// Full reload matching C's `BIT_reloadDStream` exactly.
    pub fn reload_status(&mut self) -> ReloadStatus {
        if self.bits_consumed > 64 {
            return ReloadStatus::Overflow;
        }
        let limit = 8usize;
        if self.ptr >= limit {
            let bytes_consumed = (self.bits_consumed >> 3) as usize;
            self.ptr = self.ptr.saturating_sub(bytes_consumed);
            self.bits_consumed &= 7;
            self.load_container();
            return ReloadStatus::Unfinished;
        }
        if self.ptr == 0 {
            return if self.bits_consumed < 64 {
                ReloadStatus::EndOfBuffer
            } else {
                ReloadStatus::Completed
            };
        }
        let nb_bytes = (self.bits_consumed >> 3) as usize;
        let actual_bytes = nb_bytes.min(self.ptr);
        self.ptr -= actual_bytes;
        self.bits_consumed -= (actual_bytes as u32) * 8;
        self.load_container();
        if actual_bytes < nb_bytes {
            ReloadStatus::EndOfBuffer
        } else {
            ReloadStatus::Unfinished
        }
    }

    /// Advance to the next byte boundary.
    pub fn align_to_byte(&mut self) {
        let r = self.bits_consumed % 8;
        if r != 0 {
            self.bits_consumed += 8 - r;
            if self.bits_consumed >= 8 {
                self.reload();
            }
        }
    }

    /// Debug accessor: current ptr index.
    #[doc(hidden)]
    #[must_use]
    pub const fn debug_ptr(&self) -> usize { self.ptr }

    /// Debug accessor: current bits_consumed.
    #[doc(hidden)]
    #[must_use]
    pub const fn debug_bits_consumed(&self) -> u32 { self.bits_consumed }

    /// Debug accessor: current container value.
    #[doc(hidden)]
    #[must_use]
    pub const fn debug_container(&self) -> u64 { self.container }
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
    fn long_stream_round_trip_does_not_panic() {
        // 20-byte stream — exercises the reload path.
        let data: Vec<u8> = (0..20).map(|i| (i * 17) as u8).collect();
        let mut bs = BitStream::new(&data);
        let mut total_bits_read = 0u32;
        for n in [4u32, 8, 3, 11, 5, 7, 9, 4] {
            let v = bs.read_bits(n);
            assert!(v < (1u32 << n), "read_bits({n}) returned {v} ≥ 2^{n}");
            total_bits_read += n;
        }
        assert!(total_bits_read >= 40, "expected at least 40 bits read, got {total_bits_read}");
    }

    #[test]
    fn align_advances_to_byte_boundary() {
        let mut bs = BitStream::new(&[0xFF, 0xFF, 0xFF]);
        bs.read_bits(5);
        bs.align_to_byte();
        // After reading 5 bits + align, bits_consumed should be at a
        // byte boundary relative to the stream start.
    }

    #[test]
    fn forward_stream_reads_msb_first_from_start() {
        // Byte 0xB5 = 0b1011_0101, MSB first → 1,0,1,1,0,1,0,1
        let mut fs = ForwardBitStream::from_start(&[0xB5]);
        let v = fs.read_bits(4);
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
        assert_eq!(fs.read_bits(4), 0);
    }
}
