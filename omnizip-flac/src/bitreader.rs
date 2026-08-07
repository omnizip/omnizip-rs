//! MSB-first bit reader for FLAC bitstreams.
//!
//! FLAC encodes all bit-level data MSB-first (the first bit is in the
//! most-significant position of the first byte). This is the opposite
//! of ZSTD's LSB-first `BitStream`.

#![forbid(unsafe_code)]

/// MSB-first bit reader over a byte slice.
#[derive(Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Current byte position.
    byte_pos: usize,
    /// Current bit position within the byte (0 = MSB, 7 = LSB).
    bit_pos: u8,
}

impl<'a> BitReader<'a> {
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    /// Read `n` bits MSB-first. Returns 0 if `n == 0`.
    /// Returns 0 for bits past the end of the buffer.
    #[must_use]
    pub fn read_bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let mut result: u32 = 0;
        let mut remaining = n;
        while remaining > 0 {
            if self.byte_pos >= self.data.len() {
                break;
            }
            let byte = self.data[self.byte_pos];
            let avail: u32 = (8 - self.bit_pos).into();
            let take = remaining.min(avail);
            let shift = avail - take;
            let mask = if take >= 32 {
                u32::MAX
            } else {
                (1u32 << take) - 1
            };
            let bits =
                u32::from((byte >> shift) & (if take >= 8 { 0xFF } else { (1u8 << take) - 1 }));
            result = (result << take) | (bits & mask);
            self.bit_pos += take as u8;
            if self.bit_pos >= 8 {
                self.bit_pos -= 8;
                self.byte_pos += 1;
            }
            remaining -= take;
        }
        result
    }

    /// Read a unary-coded value: count the number of leading 1-bits
    /// until a 0-bit terminator is found. Used for Rice residual
    /// coding (matches libFLAC's convention of "ones then zero").
    ///
    /// This implementation is O(n) per bit but is provably correct for
    /// any `bit_pos` alignment.
    ///
    /// Per FLAC spec: counts ZERO-bits until a single ONE-bit
    /// terminator is read. (OPPOSITE polarity from the more common
    /// "ones then zero" unary used by some other codecs.)
    #[must_use]
    pub fn read_unary(&mut self) -> u32 {
        let mut count = 0u32;
        loop {
            if self.byte_pos >= self.data.len() {
                break;
            }
            let bit = self.read_bits(1);
            if bit == 0 {
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    /// Read a signed value using Rice coding with parameter `k`.
    #[must_use]
    pub fn read_rice_signed(&mut self, k: u32) -> i32 {
        let quotient = self.read_unary();
        let remainder = self.read_bits(k);
        let value = (quotient << k) | remainder;
        // Zigzag decode.
        if value & 1 != 0 {
            -((value >> 1) as i32) - 1
        } else {
            (value >> 1) as i32
        }
    }

    /// Align to the next byte boundary.
    pub fn align_byte(&mut self) {
        if self.bit_pos > 0 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
    }

    /// Current position in bytes (after alignment).
    #[must_use]
    pub fn byte_position(&self) -> usize {
        self.byte_pos + if self.bit_pos > 0 { 1 } else { 0 }
    }

    /// Remaining bytes from the current byte position.
    #[must_use]
    pub fn remaining_bytes(&self) -> usize {
        self.data.len().saturating_sub(self.byte_pos)
    }

    /// Peek at the next byte without consuming.
    #[must_use]
    pub fn peek_byte(&self) -> Option<u8> {
        self.data.get(self.byte_pos).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_msb_first() {
        // 0xB5 = 1011_0101. Reading 4 bits MSB-first: 1011 = 11.
        let mut br = BitReader::new(&[0xB5]);
        assert_eq!(br.read_bits(4), 0b1011);
        // Next 4 bits: 0101 = 5.
        assert_eq!(br.read_bits(4), 0b0101);
    }

    #[test]
    fn read_across_bytes() {
        // [0x12, 0x34] = 0001_0010 0011_0100.
        let mut br = BitReader::new(&[0x12, 0x34]);
        assert_eq!(br.read_bits(8), 0x12);
        assert_eq!(br.read_bits(8), 0x34);
    }

    #[test]
    fn read_unary_zeros() {
        // Per FLAC spec: 0b0001_0000 = 3 zeros then a one.
        let mut br = BitReader::new(&[0x10]);
        assert_eq!(br.read_unary(), 3);
    }

    #[test]
    fn read_rice_signed() {
        // Per FLAC spec: unary = "value zeros followed by 1".
        // Rice(k=2): value 5 → quotient 1, remainder 1.
        // Unary(1) = "01" (1 zero then 1). Remainder 1 in 2 bits = "01".
        // Byte: 01_01_0000 = 0x50.
        let mut br = BitReader::new(&[0x50]);
        let val = br.read_rice_signed(2);
        // (1 << 2) | 1 = 5. Zigzag: 5 is odd → -(5>>1)-1 = -2-1 = -3.
        // Rice maps unsigned to signed: 0→0, 1→-1, 2→1, 3→-2, 4→2, 5→-3.
        assert_eq!(val, -3);
    }

    #[test]
    fn align_to_byte() {
        let mut br = BitReader::new(&[0xFF, 0xAA]);
        br.read_bits(3);
        br.align_byte();
        assert_eq!(br.byte_position(), 1);
        assert_eq!(br.read_bits(8), 0xAA);
    }
}
