//! RAR5 LZ decompression (methods 1-5) in pure Rust, ported from the
//! RAR5 format as implemented in libarchive's
//! archive_read_support_format_rar5.c (Antoniak, BSD-2) — no external
//! decompressor is ever invoked. Covers: the 20-entry nibble-coded
//! bit-length alphabet, the 430-entry table stream (NC=306 literals,
//! DC=64 distances, LDC=16 low-distance, RC=44 repeat-lengths),
//! canonical Huffman decoding with quick-nibble acceleration, the
//! distance cache, length codes, delta/E8/E8E9/ARM filters, sliding
//! window, and solid-archive window carry-over.
#![forbid(unsafe_code)]

use omnizip_archive_core::ArchiveError;

const HUFF_BC: usize = 20;
const HUFF_NC: usize = 306;
const HUFF_DC: usize = 64;
const HUFF_LDC: usize = 16;
const HUFF_RC: usize = 44;
const HUFF_TABLE_SIZE: usize = HUFF_NC + HUFF_DC + HUFF_LDC + HUFF_RC;

const WINDOW_BASE: usize = 0x20000; // 128 KB, shifted by comp-info dict code

#[derive(Clone, Debug)]
pub struct DecodeTable {
    size: usize,
    decode_len: [u32; 16],
    decode_pos: [u32; 16],
    quick_bits: u32,
    quick_len: [u8; 1 << 10],
    quick_num: [u16; 1 << 10],
    decode_num: [u16; HUFF_NC],
}

impl Default for DecodeTable {
    fn default() -> Self {
        Self::new()
    }
}

impl DecodeTable {
    fn new() -> Self {
        Self {
            size: 0,
            decode_len: [0; 16],
            decode_pos: [0; 16],
            quick_bits: 7,
            quick_len: [0; 1 << 10],
            quick_num: [0; 1 << 10],
            decode_num: [0; HUFF_NC],
        }
    }
}

fn create_decode_tables(bit_length: &[u8], size: usize) -> Result<DecodeTable, ArchiveError> {
    let mut table = DecodeTable::new();
    let mut lc = [0u32; 16];
    table.size = size;
    table.quick_bits = if size == HUFF_NC { 10 } else { 7 };

    for &bl in &bit_length[..size] {
        lc[(bl & 15) as usize] += 1;
    }
    lc[0] = 0;
    table.decode_pos[0] = 0;
    table.decode_len[0] = 0;
    let mut upper_limit: u32 = 0;
    for i in 1..16 {
        upper_limit += lc[i];
        table.decode_len[i] = upper_limit.wrapping_shl((16 - i) as u32);
        table.decode_pos[i] = table.decode_pos[i - 1] + lc[i - 1];
        upper_limit = upper_limit.wrapping_mul(2);
    }
    if upper_limit > 65536 {
        return Err(invalid("rar5: over-subscribed huffman table"));
    }

    let mut pos_clone = table.decode_pos;
    for (i, &bl) in bit_length.iter().enumerate().take(size) {
        let clen = (bl & 15) as usize;
        if clen > 0 {
            let last_pos = pos_clone[clen] as usize;
            table.decode_num[last_pos] = i as u16;
            pos_clone[clen] += 1;
        }
    }

    let quick_size = 1usize << table.quick_bits;
    let mut cur_len: usize = 1;
    for code in 0..quick_size {
        let bit_field = (code as u32) << (16 - table.quick_bits);
        // cur_len can reach 16 on sparse tables (matches the reference).
        while cur_len < 16 && bit_field >= table.decode_len[cur_len] {
            cur_len += 1;
        }
        table.quick_len[code] = cur_len as u8;
        let dist = bit_field.wrapping_sub(table.decode_len[cur_len - 1]);
        let dist = dist >> (16 - cur_len);
        let pos = table.decode_pos[cur_len & 15] as i64 + dist as i64;
        table.quick_num[code] = if cur_len < 16 && pos >= 0 && (pos as usize) < size {
            table.decode_num[pos as usize]
        } else {
            0
        };
    }
    Ok(table)
}

/// MSB-first bit reader over a padded block buffer.
struct BitReader<'a> {
    buf: &'a [u8],
    in_addr: usize,
    bit_addr: u32,
}

impl<'a> BitReader<'a> {
    fn bits16(&self, limit: usize) -> Result<u16, ArchiveError> {
        if self.in_addr >= limit {
            return Err(invalid("rar5: premature end of stream"));
        }
        let i = self.in_addr;
        let b =
            ((self.buf[i] as u32) << 16) | ((self.buf[i + 1] as u32) << 8) | self.buf[i + 2] as u32;
        Ok(((b >> (8 - self.bit_addr)) & 0xffff) as u16)
    }

    fn bits32(&self, limit: usize) -> Result<u32, ArchiveError> {
        if self.in_addr >= limit {
            return Err(invalid("rar5: premature end of stream"));
        }
        let i = self.in_addr;
        let mut bits = u32::from_be_bytes([
            self.buf[i],
            self.buf[i + 1],
            self.buf[i + 2],
            self.buf[i + 3],
        ]) << self.bit_addr;
        bits |= (self.buf[i + 4] as u32) >> (8 - self.bit_addr);
        Ok(bits)
    }

    fn skip(&mut self, bits: u32) {
        let new = self.bit_addr + bits;
        self.in_addr += (new >> 3) as usize;
        self.bit_addr = new & 7;
    }

    fn consume(&mut self, n: u32, limit: usize) -> Result<u32, ArchiveError> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.bits16(limit)? as u32;
        let v = v >> (16 - n);
        self.skip(n);
        Ok(v)
    }

    fn decode_number(&mut self, t: &DecodeTable, limit: usize) -> Result<u16, ArchiveError> {
        let bitfield = self.bits16(limit)? & 0xfffe;
        let qb = t.quick_bits as usize;
        if (bitfield as u32) < t.decode_len[qb] {
            let code = (bitfield >> (16 - t.quick_bits)) as usize;
            let len = t.quick_len[code] as u32;
            self.skip(len);
            return Ok(t.quick_num[code]);
        }
        let mut bits = 15;
        for i in (qb + 1)..15 {
            if (bitfield as u32) < t.decode_len[i] {
                bits = i;
                break;
            }
        }
        self.skip(bits as u32);
        let dist = (bitfield as u32).wrapping_sub(t.decode_len[bits - 1]) >> (16 - bits);
        let pos = t.decode_pos[bits] as u64 + dist as u64;
        let pos = if pos >= t.size as u64 {
            0
        } else {
            pos as usize
        };
        Ok(t.decode_num[pos])
    }
}

#[derive(Clone, Debug)]
struct Filter {
    kind: u16,
    channels: u32,
    block_start: u64,
    block_length: usize,
}

/// Solid-stream state carried across entries of a solid archive.
#[derive(Default)]
pub struct SolidState {
    pub window: Vec<u8>,
    pub window_size: usize,
    pub solid_offset: u64,
    /// Bytes the last unpack_lz call wrote; the caller folds this into
    /// solid_offset to advance the shared window.
    pub last_advance: u64,
    /// Huffman tables persist across blocks and — in solid streams —
    /// across files: continuation blocks arrive with table=0.
    pub bd: DecodeTable,
    pub ld: DecodeTable,
    pub dd: DecodeTable,
    pub ldd: DecodeTable,
    pub rd: DecodeTable,
    last_len: u32,
    dist_cache: [i64; 4],
}

struct Unpacker<'a> {
    solid: &'a mut SolidState,
    window_size: usize,
    unpacked_size: u64,
    write_ptr: u64,
    last_write_ptr: u64,
    filters: std::collections::VecDeque<Filter>,
    out: Vec<u8>,
    last_len: u32,
    dist_cache: [i64; 4],
}

impl<'a> Unpacker<'a> {
    fn mask(&self) -> usize {
        self.window_size - 1
    }

    fn window_byte(&self, pos: u64) -> u8 {
        self.solid.window[(pos & self.mask() as u64) as usize]
    }

    fn window_range(&self, start: u64, end: u64) -> Vec<u8> {
        let mask = self.mask() as u64;
        let mut v = Vec::with_capacity((end - start) as usize);
        let mut p = start;
        while p < end {
            v.push(self.solid.window[(p & mask) as usize]);
            p += 1;
        }
        v
    }

    fn copy_string(&mut self, len: u32, dist: i64) -> Result<(), ArchiveError> {
        if self.write_ptr > self.unpacked_size || len as u64 > self.unpacked_size - self.write_ptr {
            return Err(invalid("rar5: uncompressed data exceeds declared size"));
        }
        let base = self.solid.solid_offset + self.write_ptr;
        let mask = self.mask() as u64;
        for i in 0..len as u64 {
            let w = ((base + i) & mask) as usize;
            let r = ((base + i - dist as u64) & mask) as usize;
            self.solid.window[w] = self.solid.window[r];
        }
        self.write_ptr += len as u64;
        Ok(())
    }

    fn push_window(&mut self, start: u64, end: u64) {
        let chunk = self.window_range(start, end);
        self.out.extend_from_slice(&chunk);
        self.last_write_ptr = end;
    }

    /// Apply every filter fully covered by unpacked data; emits
    /// filtered ranges and the plain data before them, in order.
    fn emit_available(&mut self) -> Result<(), ArchiveError> {
        loop {
            let Some(f) = self.filters.front().cloned() else {
                break;
            };
            if self.write_ptr > f.block_start
                && self.write_ptr >= f.block_start + f.block_length as u64
            {
                if self.last_write_ptr == f.block_start {
                    self.run_filter(&f)?;
                    self.filters.pop_front();
                } else {
                    let to = f.block_start;
                    self.push_window(self.last_write_ptr, to);
                }
            } else {
                break;
            }
        }
        if self.filters.is_empty() && self.write_ptr > self.last_write_ptr {
            let (s, e) = (self.last_write_ptr, self.write_ptr);
            self.push_window(s, e);
        }
        Ok(())
    }

    fn run_filter(&mut self, f: &Filter) -> Result<(), ArchiveError> {
        let base = self.solid.solid_offset + f.block_start;
        let len = f.block_length;
        let mut filtered = vec![0u8; len];
        match f.kind {
            0 => {
                // delta: prev -= byte, per channel
                let mut src_pos: u64 = 0;
                for ch in 0..f.channels as u64 {
                    let mut prev: u8 = 0;
                    let mut dst = ch as usize;
                    while dst < len {
                        let b = self.window_byte(base + src_pos);
                        prev = prev.wrapping_sub(b);
                        filtered[dst] = prev;
                        src_pos += 1;
                        dst += f.channels as usize;
                    }
                }
            }
            1 | 2 => {
                filtered.copy_from_slice(&self.window_range(base, base + len as u64));
                let file_size: u32 = 0x1000000;
                let extended = f.kind == 2;
                let mut i: usize = 0;
                while i + 4 < len {
                    let b = self.window_byte(base + i as u64);
                    i += 1;
                    if b == 0xE8 || (extended && b == 0xE9) {
                        let offset = (i as u32 + f.block_start as u32) % file_size;
                        let addr = self.filter_read32(base + i as u64);
                        if addr & 0x8000_0000 != 0 {
                            if (addr.wrapping_add(offset)) & 0x8000_0000 == 0 {
                                Self::filter_write32(
                                    &mut filtered,
                                    i,
                                    addr.wrapping_add(file_size),
                                );
                            }
                        } else if (addr.wrapping_sub(file_size)) & 0x8000_0000 != 0 {
                            Self::filter_write32(&mut filtered, i, addr.wrapping_sub(offset));
                        }
                        i += 4;
                    }
                }
            }
            3 => {
                // ARM
                filtered.copy_from_slice(&self.window_range(base, base + len as u64));
                let mut i: usize = 0;
                while i + 3 < len {
                    if self.window_byte(base + (i as u64 + 3)) == 0xEB {
                        let mut offset = self.filter_read32(base + i as u64) & 0x00ff_ffff;
                        offset = offset.wrapping_sub((i as u32 + f.block_start as u32) / 4);
                        offset = (offset & 0x00ff_ffff) | 0xeb00_0000;
                        Self::filter_write32(&mut filtered, i, offset);
                    }
                    i += 4;
                }
            }
            other => {
                return Err(ArchiveError::UnsupportedFeature {
                    reason: format!("rar5: filter type {other}"),
                });
            }
        }
        self.out.extend_from_slice(&filtered);
        self.last_write_ptr += len as u64;
        Ok(())
    }

    fn filter_read32(&self, pos: u64) -> u32 {
        let b = self.window_range(pos, pos + 4);
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }

    fn filter_write32(buf: &mut [u8], at: usize, value: u32) {
        buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
}

fn invalid(msg: &str) -> ArchiveError {
    ArchiveError::InvalidArchive(msg.to_string())
}

thread_local! {
    /// Real bytes following the entry's packed data; the reference bit
    /// reader's 16/32-bit lookahead legitimately reads past the last
    /// block, so zero padding would decode a different tail.
    static LOOKAHEAD_TAIL: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[allow(dead_code)]
pub fn set_lookahead_tail(tail: Vec<u8>) {
    LOOKAHEAD_TAIL.with(|t| *t.borrow_mut() = tail);
}

/// Decode one LZ-compressed entry (methods 1..5). `solid` carries the
/// dictionary across entries of a solid archive; pass a fresh
/// `SolidState` for non-solid entries.
pub fn unpack_lz(
    packed: &[u8],
    unpacked_size: u64,
    window_size: usize,
    solid: &mut SolidState,
) -> Result<Vec<u8>, ArchiveError> {
    if window_size == 0 || !window_size.is_power_of_two() {
        return Err(invalid("rar5: invalid dictionary size"));
    }
    if solid.window.len() < window_size {
        solid.window.resize(window_size, 0);
    }
    solid.window_size = window_size;

    let mut u = Unpacker {
        last_len: solid.last_len,
        dist_cache: solid.dist_cache,
        solid,
        window_size,
        unpacked_size,
        write_ptr: 0,
        last_write_ptr: 0,
        filters: std::collections::VecDeque::new(),
        out: Vec::with_capacity(unpacked_size.min(64 << 20) as usize),
    };

    // Padded copy so bits16/bits32 can safely over-read at the tail;
    // the pad is the real bytes that follow in the archive.
    let mut padded = packed.to_vec();
    LOOKAHEAD_TAIL.with(|t| padded.extend_from_slice(&t.borrow()[..]));
    padded.extend_from_slice(&[0u8; 8]);

    let mut cursor = 0usize;
    loop {
        if u.write_ptr >= unpacked_size {
            break;
        }
        if cursor + 3 > padded.len() {
            return Err(invalid("rar5: truncated block header"));
        }
        let flags = padded[cursor];
        let cksum = padded[cursor + 1];
        let byte_count = ((flags >> 3) & 7) as usize;
        if byte_count > 2 {
            return Err(invalid("rar5: unsupported block header size"));
        }
        let block_size = match byte_count {
            0 => padded[cursor + 2] as usize,
            1 => u16::from_le_bytes([padded[cursor + 2], padded[cursor + 3]]) as usize,
            _ => {
                (u32::from_le_bytes([
                    padded[cursor + 2],
                    padded[cursor + 3],
                    padded[cursor + 4],
                    0,
                ]) & 0x00ff_ffff) as usize
            }
        };
        let calc = 0x5Au8
            ^ flags
            ^ (block_size & 0xff) as u8
            ^ ((block_size >> 8) & 0xff) as u8
            ^ ((block_size >> 16) & 0xff) as u8;
        if calc != cksum {
            return Err(ArchiveError::Checksum(format!(
                "rar5: block header checksum error (stored {cksum:02x}, computed {calc:02x})"
            )));
        }
        let table_present = flags & 0x80 != 0;
        let last_block = flags & 0x40 != 0;
        let bit_size_end = 1 + (flags & 7) as u32;
        let hdr_len = 2 + byte_count + 1;
        cursor += hdr_len;
        if cursor + block_size > padded.len() {
            return Err(invalid("rar5: block overruns data area"));
        }
        let block = padded[cursor..cursor + block_size + 8].to_vec();
        cursor += block_size;
        if block_size == 0 {
            if last_block {
                break;
            }
            continue;
        }

        let mut br = BitReader {
            buf: &block,
            in_addr: 0,
            bit_addr: 0,
        };
        let limit = block_size;

        if table_present {
            let (mut bd, mut ld, mut dd, mut ldd, mut rd) = (
                std::mem::take(&mut u.solid.bd),
                std::mem::take(&mut u.solid.ld),
                std::mem::take(&mut u.solid.dd),
                std::mem::take(&mut u.solid.ldd),
                std::mem::take(&mut u.solid.rd),
            );
            parse_tables(&mut br, limit, &mut bd, &mut ld, &mut dd, &mut ldd, &mut rd)?;
            u.solid.bd = bd;
            u.solid.ld = ld;
            u.solid.dd = dd;
            u.solid.ldd = ldd;
            u.solid.rd = rd;
        }

        loop {
            // Stop conditions mirror libarchive: emit half-window
            // chunks, and recognize the block's last bit position.
            if u.write_ptr - u.last_write_ptr > (window_size >> 1) as u64 {
                u.emit_available()?;
            }
            if br.in_addr > limit - 1 || (br.in_addr == limit - 1 && br.bit_addr >= bit_size_end) {
                break;
            }

            let num = br.decode_number(&u.solid.ld, limit)?;
            if num < 256 {
                if u.write_ptr >= unpacked_size {
                    return Err(invalid("rar5: uncompressed data exceeds declared size"));
                }
                let mask = u.mask() as u64;
                let pos = u.solid.solid_offset + u.write_ptr;
                u.solid.window[(pos & mask) as usize] = num as u8;
                u.write_ptr += 1;
            } else if num >= 262 {
                let len = decode_code_length(&mut br, limit, num - 262)?;
                let dist_slot = br.decode_number(&u.solid.dd, limit)?;
                let mut dist: i64 = 1;
                let dbits;
                if dist_slot < 4 {
                    dbits = 0;
                    dist += i64::from(dist_slot);
                } else {
                    dbits = (dist_slot / 2 - 1) as u32;
                    dist += i64::from((2 | (dist_slot & 1)) << dbits);
                }
                if dbits > 0 {
                    if dbits >= 4 {
                        if dbits > 4 {
                            let add = br.bits32(limit)?;
                            br.skip(dbits - 4);
                            let add = (add >> (36 - dbits)) << 4;
                            dist += i64::from(add);
                        }
                        let low = br.decode_number(&u.solid.ldd, limit)?;
                        dist += i64::from(low);
                    } else {
                        let add = br.consume(dbits, limit)?;
                        dist += i64::from(add);
                    }
                }
                let mut len = len;
                if dist > 0x100 {
                    len += 1;
                    if dist > 0x2000 {
                        len += 1;
                        if dist > 0x40000 {
                            len += 1;
                        }
                    }
                }
                u.dist_cache = [dist, u.dist_cache[0], u.dist_cache[1], u.dist_cache[2]];
                u.last_len = len;
                u.copy_string(len, dist)?;
            } else if num == 256 {
                parse_filter(&mut br, limit, &mut u)?;
            } else if num == 257 {
                if u.last_len != 0 {
                    let (len, dist) = (u.last_len, u.dist_cache[0]);
                    u.copy_string(len, dist)?;
                }
            } else {
                let idx = (num - 258) as usize;
                let dist = {
                    let q = &mut u.dist_cache;
                    let d = q[idx];
                    for i in (1..=idx).rev() {
                        q[i] = q[i - 1];
                    }
                    q[0] = d;
                    d
                };
                let len_slot = br.decode_number(&u.solid.rd, limit)?;
                let len = decode_code_length(&mut br, limit, len_slot)?;
                u.last_len = len;
                u.copy_string(len, dist)?;
            }
        }
        u.emit_available()?;
        if last_block || u.write_ptr >= unpacked_size {
            break;
        }
    }
    u.emit_available()?;
    // Any bytes still pending (no filters left) are the tail.
    if u.write_ptr > u.last_write_ptr && u.filters.is_empty() {
        let (s, e) = (u.last_write_ptr, u.write_ptr);
        u.push_window(s, e);
    }

    if u.out.len() as u64 != unpacked_size {
        return Err(invalid(&format!(
            "rar5: unpacked size mismatch (got {}, expected {unpacked_size})",
            u.out.len()
        )));
    }
    // Persist per-stream state for the next solid entry.
    u.solid.last_len = u.last_len;
    u.solid.dist_cache = u.dist_cache;
    u.solid.last_advance = u.write_ptr;
    Ok(u.out)
}

fn decode_code_length(br: &mut BitReader, limit: usize, code: u16) -> Result<u32, ArchiveError> {
    let (lbits, mut length) = if code < 8 {
        (0u32, 2 + u32::from(code))
    } else {
        let lbits = u32::from(code) / 4 - 1;
        let length = 2 + ((4 | (u32::from(code) & 3)) << lbits);
        (lbits, length)
    };
    if lbits > 0 {
        length += br.consume(lbits, limit)?;
    }
    Ok(length)
}

fn parse_filter(br: &mut BitReader, limit: usize, u: &mut Unpacker) -> Result<(), ArchiveError> {
    let block_start = parse_filter_data(br, limit)?;
    let block_length = parse_filter_data(br, limit)?;
    let filter_type = br.consume(3, limit)?;

    if !(4..=0x400000).contains(&block_length) || block_length as usize > u.window_size >> 1 {
        return Err(invalid("rar5: invalid filter block"));
    }
    let mut channels = 1u32;
    if filter_type == 0 {
        channels = br.consume(5, limit)? + 1;
    }
    u.filters.push_back(Filter {
        kind: filter_type as u16,
        channels,
        block_start: u.write_ptr + u64::from(block_start),
        block_length: block_length as usize,
    });
    Ok(())
}

fn parse_filter_data(br: &mut BitReader, limit: usize) -> Result<u32, ArchiveError> {
    let bytes = br.consume(2, limit)? as usize + 1;
    let mut data: u32 = 0;
    for i in 0..bytes {
        let byte = br.bits16(limit)? as u32;
        data += (byte >> 8) << (i * 8);
        br.skip(8);
    }
    Ok(data)
}

fn parse_tables(
    br: &mut BitReader,
    limit: usize,
    bd: &mut DecodeTable,
    ld: &mut DecodeTable,
    dd: &mut DecodeTable,
    ldd: &mut DecodeTable,
    rd: &mut DecodeTable,
) -> Result<(), ArchiveError> {
    // 20 bit-lengths from nibbles; 15 escapes into a zero-run.
    let mut bit_length = [0u8; HUFF_BC];
    let mut w = 0usize;
    let mut i = 0usize;
    let mut mask: u8 = 0xF0;
    let mut shift: u8 = 4;
    while w < HUFF_BC {
        if i >= limit {
            return Err(invalid("rar5: truncated huffman tables"));
        }
        let mut value = (br.buf[i] & mask) >> shift;
        if mask == 0x0F {
            i += 1;
        }
        mask ^= 0xFF;
        shift ^= 4;
        if value == 15 {
            value = (br.buf[i] & mask) >> shift;
            if mask == 0x0F {
                i += 1;
            }
            mask ^= 0xFF;
            shift ^= 4;
            if value == 0 {
                bit_length[w] = 15;
                w += 1;
            } else {
                for _ in 0..(value + 2) {
                    if w < HUFF_BC {
                        bit_length[w] = 0;
                        w += 1;
                    }
                }
            }
        } else {
            bit_length[w] = value;
            w += 1;
        }
    }
    br.in_addr = i;
    br.bit_addr = u32::from(shift ^ 4);

    *bd = create_decode_tables(&bit_length, HUFF_BC)?;

    let mut table = [0u8; HUFF_TABLE_SIZE];
    let mut ti = 0usize;
    while ti < HUFF_TABLE_SIZE {
        let num = br.decode_number(bd, limit)?;
        if num < 16 {
            table[ti] = num as u8;
            ti += 1;
        } else if num < 18 {
            let n = if num == 16 {
                br.consume(3, limit)? as usize + 3
            } else {
                br.consume(7, limit)? as usize + 11
            };
            if ti == 0 {
                return Err(invalid("rar5: repeat at table start"));
            }
            for _ in 0..n {
                if ti < HUFF_TABLE_SIZE {
                    table[ti] = table[ti - 1];
                    ti += 1;
                }
            }
        } else {
            let n = if num == 18 {
                br.consume(3, limit)? as usize + 3
            } else {
                br.consume(7, limit)? as usize + 11
            };
            for _ in 0..n {
                if ti < HUFF_TABLE_SIZE {
                    table[ti] = 0;
                    ti += 1;
                }
            }
        }
    }

    *ld = create_decode_tables(&table[0..HUFF_NC], HUFF_NC)?;
    *dd = create_decode_tables(&table[HUFF_NC..HUFF_NC + HUFF_DC], HUFF_DC)?;
    *ldd = create_decode_tables(
        &table[HUFF_NC + HUFF_DC..HUFF_NC + HUFF_DC + HUFF_LDC],
        HUFF_LDC,
    )?;
    *rd = create_decode_tables(
        &table[HUFF_NC + HUFF_DC + HUFF_LDC..HUFF_TABLE_SIZE],
        HUFF_RC,
    )?;
    Ok(())
}

/// Window size for a compression-info vint: 128 KB << dict_code.
#[must_use]
pub fn window_size_from_comp_info(ci: u64) -> usize {
    WINDOW_BASE << ((ci >> 10) & 15)
}
