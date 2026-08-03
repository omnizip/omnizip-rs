//! MSB-first bit writer for FLAC-encoded streams.
//!
//! Mirrors the [`crate::bitreader::BitReader`] — bits are packed from
//! the high end of each byte toward the low end. The first bit written
//! lands in the MSB of the first output byte.

#![forbid(unsafe_code)]

/// MSB-first bit writer. Flushes full bytes to the internal `Vec<u8>`
/// as they fill; up to 7 residual bits remain in the accumulator until
/// [`BitWriter::flush_byte_aligned`] is called.
pub struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    /// Number of valid bits in `acc`, counted from the MSB.
    bits: u8,
}

impl BitWriter {
    /// Construct an empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            acc: 0,
            bits: 0,
        }
    }

    /// Construct a writer that appends to `out` (preserving existing bytes).
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            out: Vec::with_capacity(cap),
            acc: 0,
            bits: 0,
        }
    }

    /// Write the low `count` bits of `value` (MSB-first).
    ///
    /// # Panics
    ///
    /// Panics if `count > 56` (would overflow the u64 accumulator).
    pub fn write_bits(&mut self, value: u64, count: u8) {
        assert!(count <= 56, "BitWriter.write_bits: count {count} > 56");
        if count == 0 {
            return;
        }
        let count_u32 = u32::from(count);
        let mask = if count_u32 >= 64 { u64::MAX } else { (1u64 << count_u32) - 1 };
        let v = value & mask;
        let shift = 64 - u32::from(self.bits) - count_u32;
        self.acc |= v << shift;
        self.bits += count;

        while self.bits >= 8 {
            self.out.push((self.acc >> 56) as u8);
            self.acc = self.acc.wrapping_shl(8);
            self.bits -= 8;
        }
    }

    /// Write a signed value using two's complement in `count` bits.
    pub fn write_signed(&mut self, value: i64, count: u8) {
        let unsigned = value as u64;
        self.write_bits(unsigned, count);
    }

    /// Write a unary coded value per the FLAC spec: `value` ZERO-bits
    /// followed by a single ONE-bit terminator. (Confusingly, this is
    /// the OPPOSITE of the more common "ones then zero" unary used by
    /// e.g. Golomb-Rice in other codecs — FLAC's convention is its own.)
    ///
    /// Matches the FLAC frame residual coding and wasted-bits-per-sample
    /// encoding. See libFLAC's `FLAC__bitwriter_write_unary_unsigned`.
    pub fn write_unary(&mut self, value: u32) {
        let mut remaining = value;
        // Emit `value` zero-bits in chunks of up to 56 bits.
        while remaining >= 56 {
            self.write_bits(0, 56);
            remaining -= 56;
        }
        if remaining > 0 {
            self.write_bits(0, remaining as u8);
        }
        // Terminator: a single 1-bit.
        self.write_bits(1, 1);
    }

    /// Pad the current byte with zero bits so the next write starts
    /// at a byte boundary. Matches `BitReader::align_byte`.
    pub fn flush_byte_aligned(&mut self) {
        if self.bits > 0 {
            let pad = 8 - self.bits;
            self.write_bits(0, pad);
        }
    }

    /// Write a raw byte at the current byte boundary. Requires the
    /// writer to be byte-aligned (call `flush_byte_aligned` first if
    /// unsure).
    ///
    /// # Panics
    ///
    /// Panics if `bits != 0`.
    pub fn write_byte(&mut self, byte: u8) {
        debug_assert_eq!(self.bits, 0, "write_byte requires byte alignment");
        self.out.push(byte);
    }

    /// Write a slice of raw bytes at the byte boundary.
    ///
    /// # Panics
    ///
    /// Panics if the writer is not byte-aligned.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        debug_assert_eq!(self.bits, 0, "write_bytes requires byte alignment");
        self.out.extend_from_slice(bytes);
    }

    /// Number of whole bytes written so far (excluding any bits
    /// buffered in the accumulator).
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.out.len()
    }

    /// Snapshot the current output position (byte offset + bit offset).
    /// Used by the frame encoder to compute CRC-8 of the header bytes.
    #[must_use]
    pub fn position(&self) -> usize {
        self.out.len()
    }

    /// Borrow a slice of the output bytes written so far. Only valid
    /// for the bytes already flushed (i.e., `byte_len()` bytes).
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.out
    }

    /// Finalize: pad to byte boundary and return the written bytes.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        self.flush_byte_aligned();
        self.out
    }
}

impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_byte_msb_first() {
        let mut w = BitWriter::new();
        // Write 1 (MSB), then 0, then 1 → 0b10100000 = 0xA0.
        w.write_bits(1, 1);
        w.write_bits(0, 1);
        w.write_bits(1, 1);
        let bytes = w.finish();
        assert_eq!(bytes, [0b1010_0000]);
    }

    #[test]
    fn multi_byte_continues_msb_first() {
        let mut w = BitWriter::new();
        w.write_bits(0b1010_1011, 8); // first byte
        w.write_bits(0b1100_0101, 8); // second byte
        assert_eq!(w.finish(), [0b1010_1011, 0b1100_0101]);
    }

    #[test]
    fn partial_write_flushes_correctly() {
        let mut w = BitWriter::new();
        w.write_bits(0b1, 1);
        w.write_bits(0b011, 3);
        w.write_bits(0b001, 3); // total 7 bits — no flush yet
        assert_eq!(w.byte_len(), 0);
        w.write_bits(0b1, 1); // 8 bits → flush
        assert_eq!(w.as_bytes(), [0b1011_0011]);
    }

    #[test]
    fn write_signed_handles_negative() {
        let mut w = BitWriter::new();
        // -1 in 4 bits = 0b1111.
        w.write_signed(-1, 4);
        let bytes = w.finish();
        assert_eq!(bytes[0] >> 4, 0b1111);
    }

    #[test]
    fn unary_encoding_matches_rice_convention() {
        let mut w = BitWriter::new();
        // Per FLAC spec: q=3 → three 0-bits then a 1-bit: 0b0001_0000
        w.write_unary(3);
        w.flush_byte_aligned();
        assert_eq!(w.as_bytes(), [0b0001_0000]);
    }

    #[test]
    fn align_pads_with_zeros() {
        let mut w = BitWriter::new();
        w.write_bits(0b101, 3); // 3 bits in acc
        w.flush_byte_aligned(); // pad with 5 zeros
        assert_eq!(w.as_bytes(), [0b1010_0000]);
    }
}
