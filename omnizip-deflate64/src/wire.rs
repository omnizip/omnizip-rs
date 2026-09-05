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

// ===================== Encoder =====================

/// Canonical code assignment from code lengths (RFC 1951 §3.2.2):
/// shorter codes first, ties by symbol value.
fn canonical_codes(lengths: &[u8]) -> Vec<(u16, u8, u16)> {
    let mut len_counts = [0u16; 16];
    for &l in lengths {
        if l > 0 {
            len_counts[usize::from(l)] += 1;
        }
    }
    let mut next_code = [0u16; 16];
    let mut code = 0u16;
    for l in 1..16 {
        code = (code + len_counts[l - 1]) << 1;
        next_code[l] = code;
    }
    lengths
        .iter()
        .enumerate()
        .filter(|(_, &l)| l > 0)
        .map(|(sym, &l)| {
            (sym as u16, l, {
                let c = next_code[usize::from(l)];
                next_code[usize::from(l)] += 1;
                c
            })
        })
        .collect()
}

struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    nbits: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            acc: 0,
            nbits: 0,
        }
    }
    /// LSB-first field (headers, extra bits).
    fn put(&mut self, value: u32, n: u8) {
        for i in 0..u32::from(n) {
            let bit = (value >> i) & 1;
            self.acc |= bit << self.nbits;
            self.nbits += 1;
            if self.nbits == 8 {
                self.out.push(self.acc as u8);
                self.acc = 0;
                self.nbits = 0;
            }
        }
    }
    /// Huffman code, MSB-first (RFC 1951).
    fn put_code(&mut self, code: u16, len: u8) {
        for i in (0..u32::from(len)).rev() {
            let bit = u32::from((code >> i) & 1);
            self.acc |= bit << self.nbits;
            self.nbits += 1;
            if self.nbits == 8 {
                self.out.push(self.acc as u8);
                self.acc = 0;
                self.nbits = 0;
            }
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

/// Wire length encoding under Deflate64 semantics: code 285 carries
/// 16 extra bits (base 227) — the legacy table's "285 = fixed 258"
/// is STANDARD deflate and would decode as 227 in a d64 reader.
fn wire_length_encode(length: usize) -> (usize, u32, u8) {
    let mut idx = 0usize;
    while idx < 28 {
        // length = LEN_START + 3 + extra
        let base = usize::try_from(LEN_START[idx]).unwrap_or(0) + 3;
        let bits = LEN_BITS[idx];
        let max = base + ((1u64 << u32::from(bits)) - 1) as usize;
        if length >= base && length <= max {
            return (257 + idx, (length - base) as u32, bits);
        }
        idx += 1;
    }
    // Code 285: length 227..65538 (16 extra bits).
    (285, (length - 227) as u32, 16)
}

fn wire_distance_encode(distance: usize) -> (usize, u32, u8) {
    // DIST_START is 0-based: wire distance = base + extra + 1.
    for (idx, &base0) in DIST_START.iter().enumerate() {
        let bits = DIST_BITS[idx];
        let max = usize::try_from(base0).unwrap_or(0) + ((1u64 << u32::from(bits)) - 1) as usize;
        if distance - 1 <= max {
            return (
                idx,
                (distance - 1 - usize::try_from(base0).unwrap_or(0)) as u32,
                bits,
            );
        }
    }
    (31, 0, 0)
}

/// Compress `data` (tokenized) into one final dynamic Deflate64
/// block. Deterministic: fixed tie-breaking throughout.
#[must_use]
pub fn deflate64_compress(data: &[u8], tokens: &[crate::token::Token]) -> Vec<u8> {
    let mut lit_freq = vec![0u32; 286];
    let mut dist_freq = vec![0u32; 32];
    lit_freq[256] = 1; // end-of-block
    for t in tokens {
        match *t {
            crate::token::Token::Literal { value } => lit_freq[usize::from(value)] += 1,
            crate::token::Token::Match { length, distance } => {
                let (lc, _, _) = wire_length_encode(length);
                lit_freq[lc] += 1;
                let (dc, _, _) = wire_distance_encode(distance);
                dist_freq[dc] += 1;
            }
        }
    }
    let _ = data;
    // Length-limited canonical lengths (15-bit cap).
    let mut lit_lengths = omnizip_codecs::huffman::HuffmanLengths::build(&lit_freq, 15).lengths;
    lit_lengths.resize(286, 0);
    let mut dist_lengths = omnizip_codecs::huffman::HuffmanLengths::build(&dist_freq, 15).lengths;
    dist_lengths.resize(32, 0);
    // Deflate requires at least one distance code; an all-zero table
    // (no matches) gets a dummy 1-bit code 0, mirroring zlib.
    if dist_lengths.iter().all(|&l| l == 0) {
        dist_lengths[0] = 1;
    }
    let lit_codes = canonical_codes(&lit_lengths);
    let dist_codes = canonical_codes(&dist_lengths);
    let lit_code_of = |sym: u16| lit_codes.iter().find(|(s, _, _)| *s == sym).copied();
    let dist_code_of = |sym: usize| {
        dist_codes
            .iter()
            .find(|(s, _, _)| usize::from(*s) == sym)
            .copied()
    };

    // Code-length (RLE) coding of lit||dist lengths.
    // Header trims must match the coded lengths EXACTLY: RLE-code
    // only the trimmed prefix (the previous full-table coding made the
    // decoder's HLIT+HDIST count overflow and fall through to the
    // legacy path, decoding garbage).
    let mut hlit = lit_lengths.len().min(286);
    while hlit > 257 && lit_lengths[hlit - 1] == 0 {
        hlit -= 1;
    }
    let hdist = 32usize; // full d64 alphabet — trailing zeros code cheaply via RLE 17/18

    // (symbol, payload) pairs — 16/17/18 carry a run-count payload
    // read as 2/3/7 extra bits; literals carry none.
    let mut cl_stream: Vec<(u8, u16)> = Vec::new();
    let mut all_lengths: Vec<u8> = lit_lengths[..hlit].to_vec();
    all_lengths.extend_from_slice(&dist_lengths[..hdist]);
    let mut i = 0usize;
    while i < all_lengths.len() {
        let v = all_lengths[i];
        let mut run = 1usize;
        while i + run < all_lengths.len() && all_lengths[i + run] == v {
            run += 1;
        }
        if v == 0 {
            while run >= 11 {
                let n = run.min(138);
                cl_stream.push((18, (n - 11) as u16));
                run -= n;
                i += n;
            }
            while run >= 3 {
                let n = run.min(10);
                cl_stream.push((17, (n - 3) as u16));
                run -= n;
                i += n;
            }
            for _ in 0..run {
                cl_stream.push((0, 0));
                i += 1;
            }
        } else {
            cl_stream.push((v, 0));
            i += 1;
            run -= 1;
            while run >= 3 {
                let n = run.min(6);
                cl_stream.push((16, (n - 3) as u16));
                run -= n;
                i += n;
            }
            for _ in 0..run {
                cl_stream.push((v, 0));
                i += 1;
            }
        }
    }

    let mut cl_freq = [0u32; 19];
    for &(s, _) in &cl_stream {
        cl_freq[usize::from(s)] += 1;
    }
    let mut cl_lengths = omnizip_codecs::huffman::HuffmanLengths::build(&cl_freq, 7).lengths;
    cl_lengths.resize(19, 0);
    if cl_lengths.iter().all(|&l| l == 0) {
        cl_lengths[0] = 1;
    }
    let cl_codes = canonical_codes(&cl_lengths);

    let mut hclen = 19usize;
    while hclen > 4 && cl_lengths[CODE_LENGTH_ORDER[hclen - 1]] == 0 {
        hclen -= 1;
    }

    let mut bw = BitWriter::new();
    bw.put(1, 1); // BFINAL
    bw.put(2, 2); // BTYPE = dynamic
    bw.put((hlit - 257) as u32, 5);
    bw.put((hdist - 1) as u32, 5);
    bw.put((hclen - 4) as u32, 4);
    for &ord in CODE_LENGTH_ORDER.iter().take(hclen) {
        bw.put(u32::from(cl_lengths[ord]), 3);
    }
    for &(sym, payload) in &cl_stream {
        let (_, len, code) = cl_codes
            .iter()
            .find(|(cs, _, _)| *cs == u16::from(sym))
            .copied()
            .unwrap_or((0, 0, 0));
        bw.put_code(code, len);
        match sym {
            16 => bw.put(u32::from(payload), 2),
            17 => bw.put(u32::from(payload), 3),
            18 => bw.put(u32::from(payload), 7),
            _ => {}
        }
    }
    for t in tokens {
        match *t {
            crate::token::Token::Literal { value } => {
                if let Some((_, len, code)) = lit_code_of(u16::from(value)) {
                    bw.put_code(code, len);
                }
            }
            crate::token::Token::Match { length, distance } => {
                let (lc, lextra, lbits) = wire_length_encode(length);
                if let Some((_, len, code)) = lit_code_of(lc as u16) {
                    bw.put_code(code, len);
                }
                bw.put(lextra, lbits);
                let (dc, dextra, dbits) = wire_distance_encode(distance);
                if let Some((_, len, code)) = dist_code_of(dc) {
                    bw.put_code(code, len);
                }
                bw.put(dextra, dbits);
            }
        }
    }
    if let Some((_, len, code)) = lit_code_of(256) {
        bw.put_code(code, len);
    }
    bw.finish()
}
