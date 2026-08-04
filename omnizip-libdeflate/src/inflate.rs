//! Pure-Rust RFC 1951 DEFLATE inflate.
//!
//! Phase 2 of TODO 104: in-house decoder that replaces the miniz_oxide
//! delegation. Wire format is RFC 1951 (raw DEFLATE); callers that
//! have zlib or gzip framing around it must strip the wrapper first.
//!
//! ## Algorithm
//!
//! 1. Read 3-bit block header: `BFINAL` + `BTYPE`.
//!    - `BTYPE=00`: stored (uncompressed) — copy raw bytes.
//!    - `BTYPE=01`: fixed Huffman codes — pre-defined tables.
//!    - `BTYPE=10`: dynamic Huffman — code tables in the stream.
//!    - `BTYPE=11`: reserved (error).
//! 2. Decode literals/lengths/distances using the block's Huffman
//!    tables. Drive the LZ77 back-reference loop until end-of-block.
//! 3. Repeat until the final block (`BFINAL=1`).

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation, clippy::needless_range_loop)]

use omnizip_codecs::{CodecId, OmnizipError};

/// Convenience alias for the crate's error type.
type Error = OmnizipError;

/// Build a `Corrupt` error scoped to libdeflate.
fn corrupt(reason: impl Into<String>) -> Error {
    OmnizipError::Corrupt {
        codec: CodecId::LIBDEFLATE,
        reason: reason.into(),
    }
}

/// Inflate a raw RFC 1951 DEFLATE stream into a byte vector.
///
/// `expected_len` hints the initial output capacity; the decoder
/// stops as soon as it reaches the final block (BFINAL=1).
///
/// # Errors
///
/// Returns [`OmnizipError::Corrupt`] on malformed input.
pub fn inflate(input: &[u8], expected_len: usize) -> Result<Vec<u8>, Error> {
    let mut reader = BitReader::new(input);
    let mut out = Vec::with_capacity(expected_len);

    loop {
        let bfinal = reader.read_bits(1)?;
        let btype = reader.read_bits(2)?;
        match btype {
            0 => inflate_stored(&mut reader, &mut out)?,
            1 => {
                let lit = fixed_lit_table();
                let dist = fixed_dist_table();
                inflate_block(&mut reader, &mut out, &lit, &dist)?;
            }
            2 => {
                let (lit, dist) = read_dynamic_tables(&mut reader)?;
                inflate_block(&mut reader, &mut out, &lit, &dist)?;
            }
            _ => return Err(corrupt("DEFLATE: invalid BTYPE 3 (reserved)")),
        }
        if bfinal == 1 {
            break;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Bit reader (LSB-first).
// ---------------------------------------------------------------------------

pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bits: u64,
    nbits: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, bits: 0, nbits: 0 }
    }

    #[inline]
    fn refill(&mut self, n: u32) -> Result<(), Error> {
        while self.nbits < n {
            if self.pos >= self.data.len() {
                return Err(corrupt(format!(
                    "DEFLATE: bitstream truncated (need {n} bits, have {})",
                    self.nbits
                )));
            }
            let byte = u64::from(self.data[self.pos]);
            self.pos += 1;
            self.bits |= byte << self.nbits;
            self.nbits += 8;
        }
        Ok(())
    }

    #[inline]
    fn read_bits(&mut self, n: u32) -> Result<u32, Error> {
        if n == 0 {
            return Ok(0);
        }
        self.refill(n)?;
        let mask = (1u64 << n) - 1;
        let value = (self.bits & mask) as u32;
        self.bits >>= n;
        self.nbits -= n;
        Ok(value)
    }

    #[inline]
    fn peek_bits(&mut self, n: u32) -> Result<u32, Error> {
        if n == 0 {
            return Ok(0);
        }
        self.refill(n)?;
        let mask = (1u64 << n) - 1;
        Ok((self.bits & mask) as u32)
    }

    #[inline]
    fn consume(&mut self, n: u32) {
        self.bits >>= n;
        self.nbits -= n;
    }

    fn align_to_byte(&mut self) {
        let drop = self.nbits & 7;
        self.bits >>= drop;
        self.nbits -= drop;
    }

    fn read_byte(&mut self) -> Result<u8, Error> {
        if self.pos >= self.data.len() {
            return Err(corrupt("DEFLATE: byte read past end of input"));
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }
}

// ---------------------------------------------------------------------------
// Block types.
// ---------------------------------------------------------------------------

fn inflate_stored(reader: &mut BitReader<'_>, out: &mut Vec<u8>) -> Result<(), Error> {
    reader.align_to_byte();
    let len_lo = reader.read_byte()?;
    let len_hi = reader.read_byte()?;
    let nlen_lo = reader.read_byte()?;
    let nlen_hi = reader.read_byte()?;
    let len = u16::from(len_lo) | (u16::from(len_hi) << 8);
    let nlen = u16::from(nlen_lo) | (u16::from(nlen_hi) << 8);
    if nlen != !len {
        return Err(corrupt(format!(
            "DEFLATE: stored-block NLEN ({nlen}) != ~LEN ({})",
            !len)
        ));
    }
    for _ in 0..len {
        out.push(reader.read_byte()?);
    }
    Ok(())
}

fn inflate_block(
    reader: &mut BitReader<'_>,
    out: &mut Vec<u8>,
    lit_table: &HuffmanTable,
    dist_table: &HuffmanTable,
) -> Result<(), Error> {
    loop {
        let sym = lit_table.decode(reader)? as u32;
        match sym {
            0..=255 => out.push(sym as u8),
            256 => return Ok(()),
            257..=285 => {
                let idx = (sym - 257) as usize;
                let (base_len, extra_bits) = LENGTH_BASE_EXTRA[idx];
                let extra = if extra_bits > 0 { reader.read_bits(extra_bits)? } else { 0 };
                let length = base_len + extra + 3;

                let dist_sym = dist_table.decode(reader)? as usize;
                if dist_sym >= DIST_BASE_EXTRA.len() {
                    return Err(corrupt(format!(
                        "DEFLATE: distance symbol {dist_sym} out of range"
                    )));
                }
                let (base_dist, extra_d) = DIST_BASE_EXTRA[dist_sym];
                let extra = if extra_d > 0 { reader.read_bits(extra_d)? } else { 0 };
                let distance = (base_dist + extra + 1) as usize;

                let start = out.len().checked_sub(distance).ok_or_else(|| {
                    corrupt(format!(
                        "DEFLATE: distance {distance} exceeds output length {}",
                        out.len()
                    ))
                })?;
                for i in 0..length as usize {
                    let b = out[start + i];
                    out.push(b);
                }
            }
            _ => return Err(corrupt(format!("DEFLATE: invalid symbol {sym} (>285)"))),
        }
    }
}

// ---------------------------------------------------------------------------
// Length/distance base+extra tables (RFC 1951 §3.2.5).
// ---------------------------------------------------------------------------

const LENGTH_BASE_EXTRA: [(u32, u32); 29] = [
    (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0),
    (0, 0), (0, 0), (0, 1), (0, 1), (0, 1), (0, 1),
    (0, 2), (0, 2), (0, 2), (0, 2), (0, 3), (0, 3),
    (0, 3), (0, 3), (0, 4), (0, 4), (0, 4), (0, 4),
    (0, 5), (0, 5), (0, 5), (0, 5), (0, 0),
];

const DIST_BASE_EXTRA: [(u32, u32); 30] = [
    (0, 0), (0, 0), (0, 0), (0, 0), (1, 0), (2, 0), (3, 0), (4, 1),
    (5, 1), (7, 2), (9, 2), (13, 3), (17, 3), (25, 4), (33, 4),
    (49, 5), (65, 5), (97, 6), (129, 6), (193, 7), (257, 7),
    (385, 8), (513, 8), (769, 9), (1025, 9), (1537, 10),
    (2049, 10), (3073, 11), (4097, 11), (6145, 12),
];

// ---------------------------------------------------------------------------
// Canonical Huffman decode table.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct HuffmanTable {
    /// Lookup: `(symbol, code_length)` indexed by the reversed next
    /// `max_bits` bits. Short codes are duplicated across all
    /// extensions.
    table: Vec<(u8, u8)>,
    max_bits: u32,
}

impl HuffmanTable {
    /// Build from per-symbol code lengths.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::Corrupt`] if the lengths don't form a
    /// valid Huffman tree.
    pub fn from_lengths(code_lengths: &[u8]) -> Result<Self, Error> {
        let max_bits = code_lengths.iter().copied().max().unwrap_or(0) as usize;
        if max_bits == 0 {
            return Err(corrupt(
                "DEFLATE: empty Huffman table (all code lengths are 0)",
            ));
        }

        let mut bl_count = vec![0u32; max_bits + 1];
        for &len in code_lengths {
            if len > 0 {
                bl_count[len as usize] += 1;
            }
        }

        let mut next_code = vec![0u32; max_bits + 1];
        let mut code: u32 = 0;
        for bits in 1..=max_bits {
            code = (code + bl_count[bits - 1]) << 1;
            next_code[bits] = code;
        }

        let table_size = 1usize << max_bits;
        let mut table = vec![(0u8, 0u8); table_size];
        for (sym, &len) in code_lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let len_u = len as usize;
            let canonical = next_code[len_u];
            next_code[len_u] += 1;
            let reversed = reverse_bits(canonical, len);
            let extensions = table_size >> len_u;
            let start = reversed as usize;
            for ext in 0..extensions {
                let idx = start + (ext << len_u);
                if idx >= table_size {
                    return Err(corrupt(format!(
                        "DEFLATE: Huffman table overflow at symbol {sym} (idx {idx})"
                    )));
                }
                table[idx] = (sym as u8, len);
            }
        }

        Ok(Self { table, max_bits: max_bits as u32 })
    }

    /// Decode one symbol from the bit reader.
    #[inline]
    pub(crate) fn decode(&self, reader: &mut BitReader<'_>) -> Result<u8, Error> {
        let peek = reader.peek_bits(self.max_bits)?;
        let (sym, len) = self.table[peek as usize];
        if len == 0 {
            return Err(corrupt(format!(
                "DEFLATE: Huffman lookup miss (peek={peek:#012b})"
            )));
        }
        reader.consume(u32::from(len));
        Ok(sym)
    }
}

/// Reverse the low `n` bits of `value`. Canonical Huffman codes are
/// MSB-first; the LSB-first bit reader indexes the lookup table by
/// reversed bits.
fn reverse_bits(value: u32, n: u8) -> u32 {
    let mut v = value;
    let mut r = 0u32;
    for _ in 0..n {
        r = (r << 1) | (v & 1);
        v >>= 1;
    }
    r
}

// ---------------------------------------------------------------------------
// Fixed Huffman tables (RFC 1951 §3.2.6).
// ---------------------------------------------------------------------------

use std::sync::OnceLock;

static FIXED_LIT: OnceLock<HuffmanTable> = OnceLock::new();
static FIXED_DIST: OnceLock<HuffmanTable> = OnceLock::new();

fn fixed_lit_table() -> &'static HuffmanTable {
    FIXED_LIT.get_or_init(|| {
        let mut lens = [0u8; 288];
        for i in 0..144 {
            lens[i] = 8;
        }
        for i in 144..256 {
            lens[i] = 9;
        }
        for i in 256..280 {
            lens[i] = 7;
        }
        for i in 280..288 {
            lens[i] = 8;
        }
        HuffmanTable::from_lengths(&lens)
            .expect("fixed literal/length table is valid by construction")
    })
}

fn fixed_dist_table() -> &'static HuffmanTable {
    FIXED_DIST.get_or_init(|| {
        let lens = [5u8; 30];
        HuffmanTable::from_lengths(&lens)
            .expect("fixed distance table is valid by construction")
    })
}

// ---------------------------------------------------------------------------
// Dynamic Huffman tables (RFC 1951 §3.2.7).
// ---------------------------------------------------------------------------

fn read_dynamic_tables(
    reader: &mut BitReader<'_>,
) -> Result<(HuffmanTable, HuffmanTable), Error> {
    let hlit = reader.read_bits(5)? + 257;
    let hdist = reader.read_bits(5)? + 1;
    let hclen = reader.read_bits(4)? + 4;

    const CL_ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let mut cl_lens = [0u8; 19];
    for i in 0..hclen as usize {
        cl_lens[CL_ORDER[i]] = reader.read_bits(3)? as u8;
    }
    let cl_table = HuffmanTable::from_lengths(&cl_lens)?;

    let total = (hlit + hdist) as usize;
    let mut lengths = Vec::with_capacity(total);
    while lengths.len() < total {
        let sym = cl_table.decode(reader)?;
        match sym {
            0..=15 => lengths.push(sym),
            16 => {
                let repeat = 3 + reader.read_bits(2)?;
                let prev = *lengths.last().ok_or_else(|| {
                    corrupt("DEFLATE: code-length symbol 16 has no predecessor")
                })?;
                for _ in 0..repeat {
                    lengths.push(prev);
                }
            }
            17 => {
                let repeat = 3 + reader.read_bits(3)?;
                for _ in 0..repeat {
                    lengths.push(0);
                }
            }
            18 => {
                let repeat = 11 + reader.read_bits(7)?;
                for _ in 0..repeat {
                    lengths.push(0);
                }
            }
            _ => {
                return Err(corrupt(format!(
                    "DEFLATE: invalid code-length symbol {sym}"
                )))
            }
        }
    }
    if lengths.len() > total {
        return Err(corrupt(format!(
            "DEFLATE: code-length expansion overshot ({} > {total})",
            lengths.len()
        )));
    }
    let lit_lens = &lengths[..hlit as usize];
    let dist_lens = &lengths[hlit as usize..total];
    let lit = HuffmanTable::from_lengths(lit_lens)?;
    let dist = HuffmanTable::from_lengths(dist_lens)?;
    Ok((lit, dist))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_bits_basic() {
        assert_eq!(reverse_bits(0b1101, 4), 0b1011);
        assert_eq!(reverse_bits(0b1, 1), 1);
        assert_eq!(reverse_bits(0b10, 2), 0b01);
    }

    #[test]
    fn fixed_tables_build() {
        let _ = fixed_lit_table();
        let _ = fixed_dist_table();
    }

    #[test]
    fn from_lengths_simple() {
        let lens = [2u8, 2, 2, 2];
        let table = HuffmanTable::from_lengths(&lens).expect("valid");
        assert_eq!(table.max_bits, 2);
        assert_eq!(table.table.len(), 4);
    }

    #[test]
    fn empty_lengths_error() {
        let lens = [0u8; 4];
        let err = HuffmanTable::from_lengths(&lens).unwrap_err();
        let _ = err;
    }

    #[test]
    fn bit_reader_lsb_first() {
        let data = [0b10110011u8, 0b11110000];
        let mut br = BitReader::new(&data);
        // First 3 bits LSB-first from 0b10110011 = 011
        assert_eq!(br.read_bits(3).unwrap(), 0b011);
        // Next 3 bits = 110
        assert_eq!(br.read_bits(3).unwrap(), 0b110);
    }
}
