//! LSB-first bit writer (Brotli's wire bit order per RFC 7932 §1).

/// Bit writer that accumulates bits LSB-first into bytes.
///
/// Brotli uses LSB-first bit packing throughout: the first bit
/// emitted occupies bit 0 of the first byte, the second bit
/// occupies bit 1, etc. This matches the decoder's `BitReader`
/// which reads bits in the same order.
pub struct BitWriter {
    /// Completed output bytes.
    pub out: Vec<u8>,
    /// Bit accumulator (holds up to 63 bits before a byte is flushed).
    pub acc: u64,
    /// Number of valid bits currently in `acc`.
    pub nbits: u32,
}

impl BitWriter {
    /// Create an empty writer.
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            acc: 0,
            nbits: 0,
        }
    }

    /// Write `n` bits of `value` (LSB-first).
    pub fn write_bits(&mut self, value: u32, n: u32) {
        if n == 0 {
            return;
        }
        debug_assert!(n <= 32);
        let mask: u64 = if n >= 32 {
            u32::MAX as u64
        } else {
            (1u64 << n) - 1
        };
        self.acc |= (u64::from(value) & mask) << self.nbits;
        self.nbits += n;
        while self.nbits >= 8 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc >>= 8;
            self.nbits -= 8;
        }
    }

    /// Pad with zero bits until the bit position is byte-aligned.
    pub fn byte_align(&mut self) {
        while self.nbits % 8 != 0 {
            self.write_bits(0, 1);
        }
    }

    /// Flush remaining bits and return the output bytes.
    pub fn flush(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc = 0;
            self.nbits = 0;
        }
        self.out
    }
}

impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}
