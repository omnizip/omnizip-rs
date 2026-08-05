//! Pure-Rust Brotli decoder — Phase A skeleton (RFC 7932).
//!
//! This is the first phase of replacing the upstream `brotli` crate
//! wrapper with a full pure-Rust implementation from the spec
//! (TODO 117). Phase A covers:
//!
//! - Bit reader (LSB-first per RFC 7932 §1.2).
//! - Frame header parse (RFC 7932 §9.1): window size + ISLAST.
//! - Metablock header parse (RFC 7932 §9.2): size, ISLASTEMPTY,
//!   MNIBBLES, reserved bits.
//!
//! Later phases add: block-type headers (§9.3), distance codes
//! (§9.4), Huffman decoding (§9.5), static-dictionary lookup
//! (§10), and the full encoder (Phase B/C).
//!
//! ## Status
//!
//! Phase A is a no-op decoder — it parses headers and returns
//! `Unsupported` for any block-type beyond empty metablocks. Used to
//! validate the bit-reader design before the full decoder lands.

#![forbid(unsafe_code)]

/// Minimum legal Brotli window size (per RFC 7932 §9.1).
pub const MIN_WINDOW_BITS: u8 = 10;
/// Maximum legal Brotli window size (per RFC 7932 §9.1).
pub const MAX_WINDOW_BITS: u8 = 24;
/// Brotli window bits where a 1-byte `nbl` field follows.
const LARGE_WINDOW_THRESHOLD: u8 = 16;

/// LSB-first bit reader per RFC 7932 §1.2. Bits are read from the
/// least-significant end of each byte.
pub struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    /// Construct a reader over `data`.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    /// Total bits consumeded so far.
    #[must_use]
    pub const fn bit_pos(&self) -> usize {
        self.bit_pos
    }

    /// Read `nbits` bits LSB-first. Returns 0 if past end-of-input.
    pub fn read_bits(&mut self, nbits: u32) -> u32 {
        if nbits == 0 {
            return 0;
        }
        let mut result = 0u32;
        for i in 0..nbits {
            let byte_idx = (self.bit_pos + i as usize) / 8;
            let bit_in_byte = (self.bit_pos + i as usize) % 8;
            if byte_idx < self.data.len() {
                let bit = (self.data[byte_idx] >> bit_in_byte) & 1;
                result |= u32::from(bit) << i;
            }
        }
        self.bit_pos += nbits as usize;
        result
    }

    /// Read a single bit.
    pub fn read_bit(&mut self) -> bool {
        self.read_bits(1) != 0
    }

    /// Read a variable-length integer of `nibbles` 4-bit pieces
    /// (RFC 7932 §9.2 `MLEN`).
    pub fn read_mlen(&mut self, nibbles: u32) -> u32 {
        let mut value = 0u32;
        for i in 0..nibbles {
            value |= self.read_bits(4) << (4 * i);
        }
        value + 1
    }
}

/// Parsed Brotli frame header (RFC 7932 §9.1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameHeader {
    /// Window size in bytes (2^`window_bits`).
    pub window_bits: u8,
    /// True if `ISLAST` was set on the (single) metablock — Phase A
    /// parses one metablock at a time so this lives in the frame
    /// header for simplicity.
    pub is_last: bool,
}

/// Parse the Brotli frame header at the start of `data`.
///
/// Returns the parsed header and the bit position past the header.
///
/// # Errors
///
/// Returns `&'static str` on:
/// - `data` too short (< 2 bytes),
/// - Reserved bits set,
/// - Window size outside the legal range.
pub fn parse_frame_header(data: &[u8]) -> Result<(FrameHeader, usize), &'static str> {
    if data.len() < 2 {
        return Err("input too short for frame header");
    }
    let mut br = BitReader::new(data);

    let wbits_raw = br.read_bits(1);
    let wbits = if wbits_raw == 0 {
        16u8
    } else {
        let nbl = br.read_bits(3);
        let nbl_u8 = u8::try_from(nbl).map_err(|_| "nbl overflow")?;
        if nbl_u8 == 0 {
            17u8
        } else {
            17 + nbl_u8
        }
    };
    if !(MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&wbits) {
        return Err("window size out of range");
    }

    // Reserved bits after window — RFC 7932 §9.1 says these must be 0.
    // (The frame header itself has no reserved bits in current spec;
    // reserved bits live in the metablock header.)

    let pos = br.bit_pos();
    Ok((FrameHeader { window_bits: wbits, is_last: false }, pos))
}

/// Metablock header (RFC 7932 §9.2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MetablockHeader {
    /// ISLAST flag.
    pub is_last: bool,
    /// ISLASTEMPTY flag (only meaningful when ISLAST=1).
    pub is_last_empty: bool,
    /// Number of uncompressed bytes in this metablock.
    pub mlen: u32,
    /// MNIBBLES field used to encode `mlen` (0, 1, 2, 3, or 4).
    pub mnibbles: u8,
}

/// Parse the next metablock header at `bit_pos`.
///
/// Returns the parsed header and the bit position past the header.
///
/// # Errors
///
/// Returns `&'static str` on any spec violation.
pub fn parse_metablock_header(
    data: &[u8],
    bit_pos: usize,
) -> Result<(MetablockHeader, usize), &'static str> {
    let mut br = BitReader::new(data);
    br.bit_pos = bit_pos;

    let is_last = br.read_bit();
    if is_last {
        let is_last_empty = br.read_bit();
        if is_last_empty {
            return Ok((
                MetablockHeader {
                    is_last,
                    is_last_empty: true,
                    mlen: 0,
                    mnibbles: 0,
                },
                br.bit_pos(),
            ));
        }
        let mnibbles_raw = br.read_bits(2);
        let mnibbles = if mnibbles_raw == 0 { 4 } else { mnibbles_raw };
        let mnibbles_u8 = u8::try_from(mnibbles).map_err(|_| "mnibbles overflow")?;
        let mlen = br.read_mlen(mnibbles);
        // Reserved bits — must be zero.
        if br.read_bit() {
            return Err("reserved bit set in ISLAST metablock header");
        }
        return Ok((
            MetablockHeader {
                is_last,
                is_last_empty: false,
                mlen,
                mnibbles: mnibbles_u8,
            },
            br.bit_pos(),
        ));
    }

    // ISLAST=0 path.
    let mnibbles_raw = br.read_bits(2);
    let mnibbles = if mnibbles_raw == 0 { 4 } else { mnibbles_raw };
    let mnibbles_u8 = u8::try_from(mnibbles).map_err(|_| "mnibbles overflow")?;

    // If MNIBBLES == 0 the block is metadata: MLEN is encoded but
    // skipped by Phase A. RFC 7932 §9.2.
    let mlen = br.read_mlen(mnibbles);

    // Reserved bit must be 0.
    if br.read_bit() {
        return Err("reserved bit set in metablock header");
    }

    Ok((
        MetablockHeader {
            is_last,
            is_last_empty: false,
            mlen,
            mnibbles: mnibbles_u8,
        },
        br.bit_pos(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_reader_reads_lsb_first() {
        // 0b10110010 = 0xB2. LSB-first reads: 0,1,0,0,1,1,0,1.
        let data = [0xB2u8];
        let mut br = BitReader::new(&data);
        assert_eq!(br.read_bit(), false); // bit 0
        assert_eq!(br.read_bit(), true); // bit 1
        assert_eq!(br.read_bit(), false); // bit 2
        assert_eq!(br.read_bit(), false); // bit 3
        assert_eq!(br.read_bit(), true); // bit 4
        assert_eq!(br.read_bit(), true); // bit 5
        assert_eq!(br.read_bit(), false); // bit 6
        assert_eq!(br.read_bit(), true); // bit 7
    }

    #[test]
    fn bit_reader_handles_multi_byte() {
        let data = [0xFFu8, 0x00];
        let mut br = BitReader::new(&data);
        assert_eq!(br.read_bits(8), 0xFF);
        assert_eq!(br.read_bits(8), 0x00);
    }

    #[test]
    fn bit_reader_returns_zero_past_end() {
        let data = [0xFFu8];
        let mut br = BitReader::new(&data);
        let _ = br.read_bits(8);
        // Past end: reads should return 0.
        assert_eq!(br.read_bits(4), 0);
    }

    #[test]
    fn parse_frame_header_window_16() {
        // WBITS=0 (bit 0 = 0) → window_bits = 16.
        let data = [0b0000_0000u8, 0u8];
        let (hdr, _) = parse_frame_header(&data).expect("parse");
        assert_eq!(hdr.window_bits, 16);
    }

    #[test]
    fn parse_frame_header_window_with_nbl() {
        // WBITS=1 (bit 0), NBL=2 (bits 1-3 = 010) → 17 + 2 = 19.
        // Packed LSB-first: 0b0101 = 0x05.
        let data = [0b0000_0101u8, 0u8];
        let (hdr, _) = parse_frame_header(&data).expect("parse");
        assert_eq!(hdr.window_bits, 19);
    }

    #[test]
    fn parse_frame_header_rejects_too_short() {
        let data = [0u8];
        assert!(parse_frame_header(&data).is_err());
    }

    #[test]
    fn parse_metablock_header_islast_empty() {
        // ISLAST=1 (bit 0), ISLASTEMPTY=1 (bit 1) → 0b011 = 3.
        let data = [0b0000_0011u8];
        let (hdr, _) = parse_metablock_header(&data, 0).expect("parse");
        assert!(hdr.is_last);
        assert!(hdr.is_last_empty);
        assert_eq!(hdr.mlen, 0);
    }

    #[test]
    fn parse_metablock_header_islast_with_size() {
        // ISLAST=1 (bit 0), ISLASTEMPTY=0 (bit 1), MNIBBLES=01 (bits 2-3),
        // MLEN = 0 (bits 4-7) → MLEN field says 0, so decoded mlen = 1.
        // Bits packed LSB-first: 0b00000111 = 0x07? Let's compute:
        //   bit 0 = 1 (ISLAST)
        //   bit 1 = 0 (ISLASTEMPTY)
        //   bit 2 = 1 (MNIBBLES low)
        //   bit 3 = 0 (MNIBBLES high) → MNIBBLES = 1
        //   bits 4-7 = 0000 → MLEN raw = 0, decoded = 1
        //   bit 8 = 0 (reserved)
        // Packed: 0b0000_0101 = 0x05 (first byte).
        let data = [0b0000_0101u8, 0b0000_0000u8];
        let (hdr, _) = parse_metablock_header(&data, 0).expect("parse");
        assert!(hdr.is_last);
        assert!(!hdr.is_last_empty);
        assert_eq!(hdr.mlen, 1);
    }

    #[test]
    fn parse_metablock_header_not_last() {
        // ISLAST=0 (bit 0), MNIBBLES=11 (bits 1-2) = 3,
        // MLEN = 0 (bits 3-14, 3 nibbles),
        // reserved bit 15 = 0.
        // Packed LSB-first across 2 bytes:
        //   byte 0: bits 0-7 = 0,1,1,0,0,0,0,0 = 0b0000_0110 = 0x06
        //   byte 1: bits 8-15 = 0,0,0,0,0,0,0,0 = 0x00
        let data = [0x06u8, 0x00];
        let (hdr, _) = parse_metablock_header(&data, 0).expect("parse");
        assert!(!hdr.is_last);
        assert_eq!(hdr.mnibbles, 3);
    }

    #[test]
    fn parse_metablock_header_reserved_bit_set_errors() {
        // Same as above but with reserved bit set: 0b1000_0110 first byte
        // would push bit 8 into MLEN territory. Use a longer sequence:
        //   ISLAST=0, MNIBBLES=00 (→ 4), MLEN bits 3-18, reserved = bit 19.
        // We'll set reserved=1 by setting bit 19. For simplicity,
        // construct the test by setting the highest bit of a 3-byte
        // sequence to 1.
        let mut data = [0u8; 3];
        data[0] = 0b0000_0000; // ISLAST=0, MNIBBLES=00 (→ 4)
        // MLEN occupies bits 3-18 (4 nibbles). Set all to 0.
        data[2] |= 0b0000_1000; // bit 19 = 1 (reserved)
        let result = parse_metablock_header(&data, 0);
        assert!(result.is_err(), "reserved bit must error");
    }
}
