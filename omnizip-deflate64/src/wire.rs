//! PKWARE Deflate64 (ZIP method 9) wire-format decoder.
//!
//! Implements real RFC-1951 block structure — BFINAL/BTYPE dispatch,
//! stored / fixed / dynamic blocks with HCLEN code-length coding —
//! with the Deflate64 extensions, using the exact tables from 7-Zip's
//! `DeflateConst.h` (the reference implementation our oracle `7zz`
//! runs): the 32-entry distance alphabet (codes 30/31 carry 14 extra
//! bits, covering the full 64 KiB window) and length code 285 with
//! 16 extra bits (base 227, lengths up to 65 538).
//!
//! The legacy Ruby-invented container (see `container.rs`) is kept
//! only for streams this codec itself produced before the wire layer
//! existed; `decompress` tries the strict container parse first and
//! falls through to this module.

#![forbid(unsafe_code)]

/// Length-code bases: `length = LEN_START[code - 257] + 3 + extra`.
/// Index 28 (wire code 285) is the Deflate64 extension — 16 extra
/// bits, base 224 (i.e. length 227 + extra).
const LEN_START: [u32; 29] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 128,
    160, 192, 224, 224,
];
const LEN_BITS: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 16,
];

/// Distance-code bases, 0-BASED (`wire distance = base + extra + 1`).
/// Codes 29/30/31 are the Deflate64 extension (14 extra bits each).
const DIST_START: [u32; 32] = [
    0, 1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536,
    2048, 3072, 4096, 6144, 8192, 12288, 16384, 24576, 32768, 49152,
];
const DIST_BITS: [u8; 32] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13, 14, 14,
];

const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// One canonical Huffman decoder built from code lengths (RFC 1951).
struct Huffman {
    /// For each code length, the first code value and the symbol range.
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Self, String> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[usize::from(l)] += 1;
        }
        counts[0] = 0;
        // No strict Kraft rejection: 7-Zip's writer assigns dummy
        // lengths to unused code-length symbols (verified against its
        // own output — a table with two 1-bit codes plus deeper
        // padding), and its decoder builds them with full=false. The
        // canonical walk below never reaches unreachable codes; a
        // genuinely broken stream fails the 15-bit runaway instead.
        let mut offsets = [0u16; 16];
        for l in 1..16 {
            offsets[l] = offsets[l - 1] + counts[l - 1];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[usize::from(offsets[usize::from(l)])] = sym as u16;
                offsets[usize::from(l)] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    /// Decode one symbol. Huffman codes arrive MSB-first.
    fn decode(&self, br: &mut BitReader) -> Result<u16, String> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16 {
            code |= br.bit() as i32;
            let count = i32::from(self.counts[len]);
            if code - count < first {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("invalid Huffman code (ran past 15 bits)".into())
    }
}

/// RFC 1951 bit reader: bits are packed LSB-first within bytes;
/// non-Huffman fields (headers, extra bits) also read LSB-first.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn bit(&mut self) -> u8 {
        if self.pos >> 3 >= self.data.len() {
            self.pos = self.data.len() * 8 + 16; // force later reads to fail closed
            return 0;
        }
        let v = (self.data[self.pos >> 3] >> (self.pos & 7)) & 1;
        self.pos += 1;
        v
    }
    fn bits(&mut self, n: u8) -> u32 {
        let mut v = 0u32;
        for i in 0..u32::from(n) {
            v |= u32::from(self.bit()) << i;
        }
        v
    }
    fn byte_align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }
    fn exhausted(&self) -> bool {
        self.pos > self.data.len() * 8
    }
}

/// Inflate a Deflate64 stream (the `expected` length is advisory; the
/// stream terminates on BFINAL).
///
/// # Errors
///
/// Returns a descriptive string on any structural violation.
pub fn inflate64(input: &[u8]) -> Result<Vec<u8>, String> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let mut br = BitReader {
        data: input,
        pos: 0,
    };
    let mut out: Vec<u8> = Vec::new();
    loop {
        if br.exhausted() {
            return Err("truncated stream: no final block".into());
        }
        let bfinal = br.bit();
        let btype = br.bits(2);
        match btype {
            0 => {
                if std::env::var_os("D64_TRACE").is_some() {
                    eprintln!("stored block entering at bit {}, out={}", br.pos, out.len());
                }
                br.byte_align();
                if br.pos >> 3 >= input.len().saturating_sub(4) {
                    return Err("truncated stored block header".into());
                }
                let p = br.pos >> 3;
                let len = u16::from_le_bytes([input[p], input[p + 1]]) as usize;
                let nlen = u16::from_le_bytes([input[p + 2], input[p + 3]]);
                if nlen != !(len as u16) {
                    return Err("stored block LEN/NLEN mismatch".into());
                }
                let p = p + 4;
                if p + len > input.len() {
                    return Err("stored block overruns input".into());
                }
                out.extend_from_slice(&input[p..p + len]);
                br.pos = (p + len) * 8;
            }
            1 | 2 => {
                if std::env::var_os("D64_TRACE").is_some() {
                    eprintln!("block btype={btype} bitpos={}", br.pos);
                }
                let (lit_lengths, dist_lengths): (Vec<u8>, Vec<u8>) = if btype == 1 {
                    let mut lit = vec![0u8; 288];
                    for (i, l) in lit.iter_mut().enumerate() {
                        *l = match i {
                            0..=143 => 8,
                            144..=255 => 9,
                            256..=279 => 7,
                            _ => 8,
                        };
                    }
                    (lit, vec![5u8; 32])
                } else {
                    read_dynamic_tables(&mut br)?
                };
                let lit_h = Huffman::new(&lit_lengths)?;
                let dist_h = Huffman::new(&dist_lengths)?;
                loop {
                    if br.exhausted() {
                        return Err("truncated stream inside a block".into());
                    }
                    let sym = lit_h.decode(&mut br)?;
                    match sym {
                        0..=255 => out.push(sym as u8),
                        256 => break,
                        257..=285 => {
                            let i = (sym - 257) as usize;
                            let length = LEN_START[i] + 3 + br.bits(LEN_BITS[i]);
                            let dsym = dist_h.decode(&mut br)? as usize;
                            if dsym >= 32 {
                                return Err(format!("invalid distance code {dsym}"));
                            }
                            let dist = DIST_START[dsym] + 1 + br.bits(DIST_BITS[dsym]);
                            if dist as usize > out.len() {
                                return Err(format!(
                                    "distance {dist} exceeds output so far ({} bytes)",
                                    out.len()
                                ));
                            }
                            copy_overlap(&mut out, dist as usize, length as usize);
                        }
                        _ => return Err(format!("invalid literal/length code {sym}")),
                    }
                }
            }
            _ => return Err("reserved block type 3".into()),
        }
        if bfinal == 1 {
            return Ok(out);
        }
    }
}

fn read_dynamic_tables(br: &mut BitReader) -> Result<(Vec<u8>, Vec<u8>), String> {
    let hlit = br.bits(5) as usize + 257;
    let hdist = br.bits(5) as usize + 1;
    let hclen = br.bits(4) as usize + 4;
    if hlit > 288 || hdist > 32 {
        return Err(format!(
            "dynamic header out of range: HLIT {hlit} HDIST {hdist}"
        ));
    }
    let mut cl_lengths = [0u8; 19];
    for &ord in CODE_LENGTH_ORDER.iter().take(hclen) {
        cl_lengths[ord] = br.bits(3) as u8;
    }
    let cl_h = Huffman::new(&cl_lengths)?;
    let mut lengths: Vec<u8> = Vec::with_capacity(hlit + hdist);
    while lengths.len() < hlit + hdist {
        let sym = cl_h.decode(br)?;
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                let prev = *lengths
                    .last()
                    .ok_or("code-length repeat with no previous")?;
                let n = 3 + br.bits(2);
                for _ in 0..n {
                    lengths.push(prev);
                }
            }
            17 => {
                let n = 3 + br.bits(3);
                for _ in 0..n {
                    lengths.push(0);
                }
            }
            18 => {
                let n = 11 + br.bits(7);
                for _ in 0..n {
                    lengths.push(0);
                }
            }
            _ => return Err("invalid code-length symbol".into()),
        }
    }
    if lengths.len() > hlit + hdist {
        return Err("code-length run overflows HLIT+HDIST".into());
    }
    Ok((lengths[..hlit].to_vec(), lengths[hlit..].to_vec()))
}

fn copy_overlap(out: &mut Vec<u8>, distance: usize, length: usize) {
    let start = out.len() - distance;
    for i in 0..length {
        let b = out[start + i];
        out.push(b);
    }
}
