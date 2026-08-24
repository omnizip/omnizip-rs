//! `.bz2` wire-format decoder — the read side of [`super::compress`].
//!
//! Parses the real bzip2 bitstream (stream header, block headers,
//! symbol maps, MTF-coded selectors, delta-coded Huffman tables,
//! RUNA/RUNB zero runs) and drives the same inverse pipeline the
//! internal codec uses: RLE2^-1, seed-aware MTF^-1, BWT^-1, RLE1^-1,
//! verifying every block CRC and the combined stream CRC.
#![forbid(unsafe_code)]

use super::crc32::crc32;
use super::BLOCK_MAGIC;
use super::EOS_MAGIC;
use crate::bwt::bwt_decode;
use crate::rle::rle_decode;
use omnizip_codecs::{CodecId, OmnizipError};

const GROUP_SIZE: usize = 50;
const MAX_GROUPS: usize = 6;
const RUNA: u16 = 0;
const RUNB: u16 = 1;

fn err(reason: impl Into<String>) -> OmnizipError {
    OmnizipError::DecodeFailed {
        codec: CodecId::BZIP2,
        reason: reason.into(),
    }
}

/// MSB-first bit reader.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bits: u64,
    nbits: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bits: 0,
            nbits: 0,
        }
    }

    fn read_bit(&mut self) -> Result<bool, OmnizipError> {
        if self.nbits == 0 {
            if self.pos >= self.data.len() {
                return Err(err("unexpected end of bitstream"));
            }
            self.bits = u64::from(self.data[self.pos]);
            self.pos += 1;
            self.nbits = 8;
        }
        self.nbits -= 1;
        Ok(self.bits & (1 << self.nbits) != 0)
    }

    fn read_bits(&mut self, n: u32) -> Result<u32, OmnizipError> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | u32::from(self.read_bit()?);
        }
        Ok(v)
    }

    fn read48(&mut self) -> Result<u64, OmnizipError> {
        Ok(u64::from(self.read_bits(24)?) << 24 | u64::from(self.read_bits(24)?))
    }

    /// Bytes consumed, rounded up to the byte holding unread bits.
    fn consumed(&self) -> usize {
        self.pos - usize::from(self.nbits > 0)
    }
}

/// One decoded Huffman table: (symbol, code, len) triples; symbols
/// are matched by walking bits one at a time and comparing the
/// accumulated (code, len) pair.
struct HufTable {
    entries: Vec<(u16, u32, u8)>,
}

impl HufTable {
    fn decode_symbol(&self, r: &mut BitReader<'_>) -> Result<u16, OmnizipError> {
        let mut code = 0u32;
        let mut len = 0u8;
        loop {
            code = (code << 1) | u32::from(r.read_bit()?);
            len += 1;
            if len > 24 {
                return Err(err("huffman code longer than 24 bits"));
            }
            for &(sym, c, l) in &self.entries {
                if l == len && c == code {
                    return Ok(sym);
                }
            }
        }
    }
}

/// Decompress a complete `.bz2` stream (single member; multi-stream
/// files are the caller's concatenation loop). Verifies block CRCs
/// and the combined stream CRC.
///
/// # Errors
///
/// [`OmnizipError::DecodeFailed`] on any malformed structure or CRC
/// mismatch.
pub fn decompress_framed(input: &[u8]) -> Result<Vec<u8>, OmnizipError> {
    // Stream header: "BZh" + level digit (exactly 32 bits, so the
    // body is byte-aligned at offset 4).
    if input.len() < 4 || &input[..3] != b"BZh" || !input[3].is_ascii_digit() {
        return Err(err("not a bzip2 stream (bad header)"));
    }
    let mut r = BitReader::new(&input[4..]);

    let mut out = Vec::new();
    let mut combined: u32 = 0;
    loop {
        let magic = r.read48()?;
        if magic == EOS_MAGIC {
            let stored = r.read_bits(32)?;
            if stored != combined {
                return Err(err(format!(
                    "combined CRC mismatch: stored {stored:08X}, computed {combined:08X}"
                )));
            }
            return Ok(out);
        }
        if magic != BLOCK_MAGIC {
            return Err(err(format!("bad block magic {magic:012X}")));
        }

        let block_crc = r.read_bits(32)?;
        let randomised = r.read_bit()?;
        if randomised {
            return Err(err("randomised blocks are not supported"));
        }
        let orig_ptr = r.read_bits(24)? as usize;

        // Symbol map: 16 group-usage bits, then a 16-bit byte map per
        // used group. The active-byte sequence is byte order.
        let groups = r.read_bits(16)?;
        if groups == 0 {
            return Err(err("empty symbol map"));
        }
        let mut sequence: Vec<u8> = Vec::with_capacity(256);
        for g in 0..16 {
            if groups & (1 << (15 - g)) != 0 {
                let map = r.read_bits(16)?;
                for b in 0..16 {
                    if map & (1 << (15 - b)) != 0 {
                        sequence.push((g * 16 + b) as u8);
                    }
                }
            }
        }
        let n_in_use = sequence.len();

        // Huffman table group count + selector count.
        let n_groups = r.read_bits(3)? as usize;
        if !(2..=MAX_GROUPS).contains(&n_groups) {
            return Err(err(format!("invalid nGroups {n_groups}")));
        }
        let n_selectors = r.read_bits(15)? as usize;
        if n_selectors == 0 {
            return Err(err("zero selectors"));
        }

        // Selectors: unary MTF values, then MTF-decoded to table ids.
        let mut selector_mtf = Vec::with_capacity(n_selectors);
        for _ in 0..n_selectors {
            let mut j = 0u8;
            while r.read_bit()? {
                j += 1;
                if usize::from(j) > MAX_GROUPS {
                    return Err(err("selector unary run too long"));
                }
            }
            selector_mtf.push(j);
        }
        let mut order: Vec<u8> = (0..n_groups as u8).collect();
        let mut selectors = Vec::with_capacity(n_selectors);
        for &j in &selector_mtf {
            let idx = usize::from(j);
            if idx >= order.len() {
                return Err(err("selector MTF index out of range"));
            }
            let table = order.remove(idx);
            order.insert(0, table);
            selectors.push(table);
        }

        // Huffman tables: 5-bit start length, then per symbol '10'
        // (+1) / '11' (-1) until '0'.
        let alphabet = n_in_use + 2;
        let mut tables = Vec::with_capacity(n_groups);
        for _ in 0..n_groups {
            let mut lengths = vec![0u8; alphabet];
            let mut cur = r.read_bits(5)? as i32;
            if !(1..=24).contains(&cur) {
                return Err(err("huffman start length out of range"));
            }
            for slot in lengths.iter_mut() {
                loop {
                    if !(1..=24).contains(&cur) {
                        return Err(err("huffman length out of range"));
                    }
                    if !r.read_bit()? {
                        break;
                    }
                    cur += if r.read_bit()? { -1 } else { 1 };
                }
                *slot = cur as u8;
            }
            // Canonical codes for this table.
            let mut entries = Vec::with_capacity(alphabet);
            let mut code = 0u32;
            let mut length = 0u8;
            // Assign codes in symbol order at each length (lengths are
            // already sorted-friendly for canonical assignment: iterate
            // lengths ascending).
            let mut order: Vec<usize> = (0..alphabet).collect();
            order.sort_by_key(|&i| lengths[i]);
            for &i in &order {
                while length < lengths[i] {
                    code <<= 1;
                    length += 1;
                }
                if lengths[i] > 0 {
                    entries.push((i as u16, code, lengths[i]));
                    code += 1;
                }
            }
            tables.push(HufTable { entries });
        }

        // Decode the symbol stream group by group until EOB.
        let eob = n_in_use as u16 + 1;
        let mut symbols: Vec<u16> = Vec::new();
        'blocks: for &sel in &selectors {
            let table = &tables[usize::from(sel)];
            for _ in 0..GROUP_SIZE {
                let sym = table.decode_symbol(&mut r)?;
                if sym == eob {
                    break 'blocks;
                }
                symbols.push(sym);
            }
        }

        // Inverse pipeline: RLE2 -> seeded MTF -> BWT -> RLE1.
        let mtf = symbols_to_mtf(&symbols, n_in_use)?;
        let bwt = mtf_decode_seeded(&mtf, &sequence);
        let block = bwt_decode(&bwt, orig_ptr as u32).map_err(|e| err(format!("bwt: {e}")))?;
        let data = rle_decode(&block).map_err(|e| err(format!("rle1: {e}")))?;
        if crc32(&data) != block_crc {
            return Err(err(format!(
                "block CRC mismatch: stored {block_crc:08X}, computed {:08X}",
                crc32(&data)
            )));
        }
        combined = combined.rotate_left(1) ^ block_crc;
        out.extend_from_slice(&data);
    }
}

/// RUNA/RUNB + symbol stream → MTF values (RLE2 inverse).
fn symbols_to_mtf(symbols: &[u16], n_in_use: usize) -> Result<Vec<u8>, OmnizipError> {
    let eob = n_in_use as u16 + 1;
    let mut mtf: Vec<u8> = Vec::with_capacity(n_in_use * 2 + 8);
    let mut i = 0usize;
    while i < symbols.len() {
        let sym = symbols[i];
        if sym == RUNA || sym == RUNB {
            let mut run: u64 = 0;
            let mut bit: u64 = 1;
            while i < symbols.len() && (symbols[i] == RUNA || symbols[i] == RUNB) {
                if symbols[i] == RUNB {
                    run += bit << 1;
                } else {
                    run += bit;
                }
                bit <<= 1;
                i += 1;
            }
            for _ in 0..run.min(1 << 24) {
                mtf.push(0);
            }
        } else if sym == eob {
            break;
        } else {
            mtf.push((sym - 1) as u8);
            i += 1;
        }
    }
    Ok(mtf)
}

/// Seed-aware MTF inverse: the MTF list starts as `sequence` (the
/// active bytes in byte order), not the full 0..255 alphabet.
fn mtf_decode_seeded(data: &[u8], sequence: &[u8]) -> Vec<u8> {
    let mut list: Vec<u8> = sequence.to_vec();
    let mut out = Vec::with_capacity(data.len());
    for &v in data {
        let idx = usize::from(v);
        if idx < list.len() {
            let b = list.remove(idx);
            list.insert(0, b);
            out.push(b);
        } else {
            out.push(0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bz2::compress;

    #[test]
    fn round_trips_wire_format() {
        let data = b"hello hello hello bzip2 wire format round trip".repeat(30);
        let bz = compress(&data, 9).unwrap();
        assert!(bz.starts_with(b"BZh9"));
        let back = decompress_framed(&bz).unwrap();
        assert_eq!(back, data);
    }
}

#[cfg(test)]
mod stage_tests {
    use super::*;
    use crate::bwt::bwt_encode;
    use crate::bz2::mtf::{build_seed, mtf_encode_with_seed};
    use crate::bz2::rle2;
    use crate::rle::rle_encode;

    #[test]
    fn inverse_pipeline_round_trips() {
        for input in [
            b"abcabcabc".as_slice(),
            b"aaaaaaaabbbbbb".as_slice(),
            (0..200u32)
                .map(|i| (i % 251) as u8)
                .collect::<Vec<_>>()
                .as_slice(),
        ] {
            let rle1 = rle_encode(input);
            let (bwt, pi) = bwt_encode(&rle1);
            let seed = build_seed(&bwt);
            let mtf = mtf_encode_with_seed(&bwt, &seed);
            let symbols = rle2::mtf_to_symbols(&mtf, seed.len());

            // reader inverse
            let n_in_use = seed.len();
            let back_mtf = super::symbols_to_mtf(&symbols[..symbols.len() - 1], n_in_use).unwrap();
            assert_eq!(back_mtf, mtf, "mtf mismatch");
            let back_bwt = super::mtf_decode_seeded(&back_mtf, &seed);
            assert_eq!(back_bwt, bwt, "bwt mismatch");
            let back_rle1 = bwt_decode(&back_bwt, pi).unwrap();
            assert_eq!(&back_rle1, &rle1, "bwt inverse mismatch");
            let back = rle_decode(&back_rle1).unwrap();
            assert_eq!(&back, input, "rle1 inverse mismatch");
        }
    }
}
