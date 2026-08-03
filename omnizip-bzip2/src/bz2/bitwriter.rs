//! MSB-first bit writer for the standard bzip2 wire format.
//!
//! bzip2 packs bits most-significant-bit-first into bytes. The
//! [`Bz2BitWriter`] accumulates bits and flushes whole bytes to an
//! internal Vec<u8>. Call [`finish`] to pad the final partial byte
//! with zero bits and return the packed bytes.

#![forbid(unsafe_code)]

/// MSB-first bit packer.
pub struct Bz2BitWriter {
    out: Vec<u8>,
    /// Current byte under construction; bits accumulate in the high bits.
    current: u64,
    /// Number of bits currently held in `current` (0..=64).
    nbits: u32,
}

impl Bz2BitWriter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            current: 0,
            nbits: 0,
        }
    }

    /// Write the low `n` bits of `bits`, MSB-first. `n` must be 0..=32
    /// (bzip2's widest field is the 32-bit trailing CRC; the 48-bit
    /// block magics are written as two 24-bit halves via [`write48`]).
    pub fn write_bits(&mut self, bits: u32, n: u32) {
        debug_assert!(n <= 32);
        if n == 0 {
            return;
        }
        let mask = if n == 32 {
            u64::from(bits)
        } else {
            u64::from(bits & ((1u32 << n) - 1))
        };
        // If we have 32+ bits pending, flush a byte to make room.
        if self.nbits + n > 64 {
            // Shouldn't happen with normal usage (nbits ≤ 7 between calls)
            // but guard anyway.
            self.nbits -= 8;
            let byte = ((self.current >> u64::from(self.nbits)) & 0xFF) as u8;
            self.out.push(byte);
        }
        // Shift the new bits above any pending ones and append.
        self.current = (self.current << u64::from(n)) | mask;
        self.nbits += n;
        while self.nbits >= 8 {
            self.nbits -= 8;
            let byte = ((self.current >> u64::from(self.nbits)) & 0xFF) as u8;
            self.out.push(byte);
        }
    }

    /// Write a single bit (1 if `bit` is true, 0 otherwise).
    pub fn write_bit(&mut self, bit: bool) {
        self.write_bits(u32::from(bit), 1);
    }

    /// Write `n` bits of a 48-bit value (used for the block magics).
    /// Splits the value into 24-bit halves internally.
    pub fn write48(&mut self, value: u64) {
        debug_assert!(value < (1u64 << 48));
        self.write_bits((value >> 24) as u32, 24);
        self.write_bits((value & 0xFF_FFFF) as u32, 24);
    }

    /// Pad any partial byte with zeros and return the packed bytes.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            let byte = ((self.current << u64::from(8 - self.nbits)) & 0xFF) as u8;
            self.out.push(byte);
        }
        self.out
    }
}

impl Default for Bz2BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_bits_is_noop() {
        let mut w = Bz2BitWriter::new();
        w.write_bits(0xFF, 0);
        let out = w.finish();
        assert!(out.is_empty());
    }

    #[test]
    fn eight_bits_make_one_byte_msb_first() {
        let mut w = Bz2BitWriter::new();
        w.write_bits(0b1010_1010, 8);
        let out = w.finish();
        assert_eq!(out, vec![0b1010_1010]);
    }

    #[test]
    fn nine_bits_make_two_bytes_padded_with_zeros() {
        let mut w = Bz2BitWriter::new();
        w.write_bits(0b1_0110_0011, 9);
        let out = w.finish();
        // High byte: 1_0110_001, low bit shifted up by 7 → 0b10110001
        // Wait, MSB-first: we have 9 bits 1_0110_0011. First byte = 10110001
        // (top 8 bits). Second byte = 1000_0000 (1 leftover bit, shifted to MSB).
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], 0b1011_0001);
        assert_eq!(out[1], 0b1000_0000);
    }

    #[test]
    fn single_bits_accumulate_msb_first() {
        let mut w = Bz2BitWriter::new();
        for bit in [true, false, true, true, false, false, true, false] {
            w.write_bit(bit);
        }
        let out = w.finish();
        assert_eq!(out, vec![0b10110010]);
    }

    #[test]
    fn write48_emits_six_bytes_be() {
        let mut w = Bz2BitWriter::new();
        w.write48(0x3141_5926_5359);
        let out = w.finish();
        assert_eq!(
            out,
            vec![0x31, 0x41, 0x59, 0x26, 0x53, 0x59]
        );
    }
}
