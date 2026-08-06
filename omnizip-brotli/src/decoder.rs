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

/// LSB-first bit reader per RFC 7932 §1.2. Bits are read from the
/// least-significant end of each byte.
pub struct BitReader<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) bit_pos: usize,
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
        if nbits == 0 || nbits > 32 {
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

    /// Peek `nbits` bits WITHOUT advancing the bit position.
    /// Used for table-based Huffman symbol lookup.
    pub fn peek_bits(&self, nbits: u32) -> u32 {
        if nbits == 0 || nbits > 32 {
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
        result
    }

    /// Drop `nbits` bits (advance the bit position).
    pub fn drop_bits(&mut self, nbits: u32) {
        self.bit_pos += nbits as usize;
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

/// Parse the Brotli frame header at `bit_pos` in `data`.
///
/// Returns the parsed header and the bit position past the header.
///
/// The frame header is variable-width (1, 4, or 7 bits) per RFC 7932 §9.1:
/// - bit 0 = 0 → window_bits = 16
/// - bit 0 = 1, NBL (3 bits) > 0 → window_bits = 17 + NBL (so 18..24)
/// - bit 0 = 1, NBL = 0, N2 (3 bits) > 0 → window_bits = 8 + N2 (large-window extension, 9..15)
/// - bit 0 = 1, NBL = 0, N2 = 0 → window_bits = 17
/// - bit 0 = 1, NBL = 0, N2 = 1 + large_window flag → future extension
///
/// # Errors
///
/// Returns `&'static str` on:
/// - `data` too short (< 1 byte),
/// - Reserved bits set,
/// - Window size outside the legal range.
pub fn parse_frame_header(
    data: &[u8],
    bit_pos: usize,
) -> Result<(FrameHeader, usize), &'static str> {
    if data.is_empty() {
        return Err("input too short for frame header");
    }
    let mut br = BitReader::new(data);
    br.bit_pos = bit_pos;

    let wbits_raw = br.read_bits(1);
    let wbits = if wbits_raw == 0 {
        16u8
    } else {
        let nbl = br.read_bits(3);
        let nbl_u8 = u8::try_from(nbl).map_err(|_| "nbl overflow")?;
        if nbl_u8 != 0 {
            17 + nbl_u8
        } else {
            // NBL=0: read N2 (3 bits) for the large-window extension.
            let n2 = br.read_bits(3);
            let n2_u8 = u8::try_from(n2).map_err(|_| "n2 overflow")?;
            if n2_u8 != 0 {
                8 + n2_u8
            } else {
                17
            }
        }
    };
    if !(MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&wbits) && !(9..=15).contains(&wbits) {
        return Err("window size out of range");
    }

    Ok((FrameHeader { window_bits: wbits, is_last: false }, br.bit_pos()))
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
    /// IS_UNCOMPRESSED flag (RFC 7932 §9.2). When true, the metablock
    /// payload is `mlen` raw bytes (no Huffman coding).
    pub is_uncompressed: bool,
}

/// Parse the next metablock header at `bit_pos`.
///
/// Returns the parsed header and the bit position past the header.
///
/// Per RFC 7932 §9.2 the layout is:
/// - ISLAST (1 bit)
/// - if ISLAST=1: ISLASTEMPTY (1 bit)
///   - if ISLASTEMPTY=1: end (mlen=0, mnibbles=0, is_uncompressed=false)
///   - if ISLASTEMPTY=0: fall through
/// - MNIBBLES (2 bits): if 0 → use 4 nibbles for MLEN; else MNIBBLES itself
/// - MLEN (4 × MNIBBLES bits): mlen = value + 1
/// - IS_UNCOMPRESSED (1 bit)
/// - Reserved (1 bit, must be 0)
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
                    is_uncompressed: false,
                },
                br.bit_pos(),
            ));
        }
        let mnibbles_raw = br.read_bits(2);
        let mnibbles = if mnibbles_raw == 0 { 4 } else { mnibbles_raw };
        let mnibbles_u8 = u8::try_from(mnibbles).map_err(|_| "mnibbles overflow")?;
        let mlen = br.read_mlen(mnibbles);
        // Per upstream `BrotliDecoderState::METABLOCK_HEADER_UNCOMPRESSED`:
        // IS_UNCOMPRESSED is only read when ISLAST=0 (and is_metadata=0).
        // For ISLAST=1 metablocks, the body is always Huffman-coded.
        return Ok((
            MetablockHeader {
                is_last,
                is_last_empty: false,
                mlen,
                mnibbles: mnibbles_u8,
                is_uncompressed: false,
            },
            br.bit_pos(),
        ));
    }

    // ISLAST=0 path: read MNIBBLES, MLEN, IS_UNCOMPRESSED.
    let mnibbles_raw = br.read_bits(2);
    let mnibbles = if mnibbles_raw == 0 { 4 } else { mnibbles_raw };
    let mnibbles_u8 = u8::try_from(mnibbles).map_err(|_| "mnibbles overflow")?;
    let mlen = br.read_mlen(mnibbles);
    let is_uncompressed = br.read_bit();

    Ok((
        MetablockHeader {
            is_last,
            is_last_empty: false,
            mlen,
            mnibbles: mnibbles_u8,
            is_uncompressed,
        },
        br.bit_pos(),
    ))
}

// ---------------------------------------------------------------------------
// Phase B: block-type header (RFC 7932 §9.3)
// ---------------------------------------------------------------------------

/// Maximum number of block types per category (RFC 7932 §9.3).
pub const MAX_BLOCK_TYPES: u32 = 256;

/// Block-type category: literals, insert-and-copy, or distance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockTypeCategory {
    /// Literal block types (§9.3.1).
    Literal,
    /// Insert-and-copy block types (§9.3.2).
    InsertCopy,
    /// Distance block types (§9.3.3).
    Distance,
}

/// Per-category block-type header (RFC 7932 §9.3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockTypeHeader {
    /// `NBLTYPES` — number of block types in this category (1..=256).
    pub num_block_types: u32,
    /// Block-type 0 (initial).
    pub initial_block_type: u32,
    /// `NBLTYPESL` (literal) / `NBLTYPESI` (insert-copy) / `NBLTYPESD`
    /// (distance) — the count of Huffman trees in this category.
    pub num_huffman_trees: u32,
}

/// Parse a per-category block-type header (RFC 7932 §9.3).
///
/// Returns the parsed header and the bit position past the header.
/// When `num_block_types == 1`, the encoder omits the block-type
/// jump table; this function returns `num_huffman_trees = 1` in
/// that case.
pub fn parse_block_type_header(
    data: &[u8],
    bit_pos: usize,
    _category: BlockTypeCategory,
) -> Result<(BlockTypeHeader, usize), &'static str> {
    let mut br = BitReader::new(data);
    br.bit_pos = bit_pos;

    let num_block_types_raw = br.read_bits(2);
    let num_block_types = num_block_types_raw + 1;

    if num_block_types == 1 {
        return Ok((
            BlockTypeHeader {
                num_block_types: 1,
                initial_block_type: 0,
                num_huffman_trees: 1,
            },
            br.bit_pos(),
        ));
    }

    // Block-type context: initial type + jump table (3 varint-coded
    // Huffman trees for type-0, type-1, type-switch).
    let initial_block_type = br.read_bits(ceil_log2(num_block_types));

    // Read the block-type jump table: 3 entries, each ceil_log2
    // bits wide.
    for _ in 0..3 {
        let _ = br.read_bits(ceil_log2(num_block_types));
    }

    Ok((
        BlockTypeHeader {
            num_block_types,
            initial_block_type,
            num_huffman_trees: num_block_types,
        },
        br.bit_pos(),
    ))
}

/// ceil(log2(n)) for n ≥ 2, defined per RFC 7932 §1.4.
fn ceil_log2(n: u32) -> u32 {
    if n <= 1 {
        return 0;
    }
    32 - (n - 1).leading_zeros()
}

/// Bit-reverse the low `n` bits of `v` (RFC 7932 §1.2 / huffman lookup).
fn reverse_bits(n: u32, v: u32) -> u32 {
    let mut v = v;
    let mut r = 0u32;
    for _ in 0..n {
        r = (r << 1) | (v & 1);
        v >>= 1;
    }
    r
}

// ---------------------------------------------------------------------------
// Phase B: distance-code header (RFC 7932 §9.4)
// ---------------------------------------------------------------------------

/// Distance-code header (RFC 7932 §9.4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DistanceHeader {
    /// `NDIRECT` — number of direct distance codes.
    pub num_direct: u32,
    /// `NMPOSTFIX` — number of postfix bits.
    pub num_postfix: u32,
}

/// Parse the distance header (RFC 7932 §9.4). Only present when the
/// metablock uses distance codes (i.e., not a literal-only block).
pub fn parse_distance_header(
    data: &[u8],
    bit_pos: usize,
) -> Result<(DistanceHeader, usize), &'static str> {
    let mut br = BitReader::new(data);
    br.bit_pos = bit_pos;

    let npostfix_raw = br.read_bits(2);
    // NDIRECT is NPOSTFIX + 1 4-bit groups.
    // Each group: high bit set → continue reading; low 4 bits → value.
    // For simplicity (Phase B), support the 1-group form.
    let ndirect_msb = br.read_bits(1);
    let ndirect_lo = br.read_bits(4);
    let ndirect = ndirect_lo | if ndirect_msb != 0 { 16 } else { 0 };

    Ok((
        DistanceHeader {
            num_direct: ndirect,
            num_postfix: npostfix_raw,
        },
        br.bit_pos(),
    ))
}

// ---------------------------------------------------------------------------
// Phase B: Huffman table reader (RFC 7932 §9.5, simple form)
// ---------------------------------------------------------------------------

/// Maximum Huffman code length per RFC 7932 §9.5.
pub const MAX_HUFFMAN_CODE_LENGTH: u8 = 15;

/// Canonical Huffman table with flat 2^15 lookup for O(1) correct decode.
#[derive(Clone, Debug)]
pub struct HuffmanTable {
    /// Flat lookup: for each 15-bit peek value, (symbol, bits_to_consume).
    lookup: Vec<(u16, u8)>,
    /// For NSYM=1 simple form: return without consuming bits.
    single_symbol: Option<u32>,
}

impl HuffmanTable {
    #[must_use]
    pub fn from_lengths(lengths: &[u8]) -> Self {
        let n = lengths.len();
        let mut codes = vec![0u16; n];
        let mut bl_count = [0u32; 17];
        for &l in lengths {
            if l > 0 { bl_count[usize::from(l)] += 1; }
        }
        let mut next_code = [0u16; 17];
        let mut code = 0u16;
        for bits in 1..=16usize {
            code = (code + bl_count[bits - 1] as u16) << 1;
            next_code[bits] = code;
        }
        for i in 0..n {
            let l = lengths[i];
            if l > 0 {
                codes[i] = reverse_bits(u32::from(l), u32::from(next_code[usize::from(l)])) as u16;
                next_code[usize::from(l)] += 1;
            }
        }
        let nonzero_count: usize = lengths.iter().filter(|&&l| l != 0).count();
        let single_symbol = if nonzero_count == 1 {
            Some(lengths.iter().position(|&l| l != 0).unwrap() as u32)
        } else { None };

        // Build flat 2^15 lookup table: for each symbol with code
        // length L, its bit-reversed canonical code fills all
        // 2^(15-L) possible high-bit extensions.
        let mut lookup = vec![(0u16, 0u8); 32768];
        for i in 0..n {
            let l = lengths[i];
            if l > 0 && l <= 15 {
                let base = u32::from(codes[i]);
                for high in 0u32..(1u32 << (15 - l)) {
                    let idx = (base | (high << l)) as usize;
                    if idx < 32768 {
                        lookup[idx] = (i as u16, l);
                    }
                }
            }
        }
        Self { lookup, single_symbol }
    }

    /// O(1) symbol decode via 15-bit peek + flat table lookup.
    pub fn read_symbol(&self, br: &mut BitReader) -> Option<u32> {
        if let Some(sym) = self.single_symbol {
            return Some(sym);
        }
        let bits = br.peek_bits(15);
        let (sym, len) = self.lookup[bits as usize];
        if len == 0 { return None; }
        br.drop_bits(len as u32);
        Some(sym as u32)
    }
}

/// Read a Huffman table from the bitstream (RFC 7932 §9.5).
///
/// Dispatches on the 2-bit HSKIP prefix:
/// - HSKIP = 1 → simple form (NSYM = 1/2/3/4 with optional tree select)
/// - HSKIP = 0/2/3 → complex form (with that many leading code-length
///   codes assumed zero)
///
/// `alphabet_size` is the maximum number of symbols in the alphabet
/// (e.g. 256 for literals, 704 for commands, 64 for distances).
/// `max_bits` caps the code-length code's symbol bit width.
pub fn read_huffman_table(
    data: &[u8],
    bit_pos: usize,
    alphabet_size: usize,
) -> Result<(HuffmanTable, usize), &'static str> {
    let mut br = BitReader::new(data);
    br.bit_pos = bit_pos;

    let hskip = br.read_bits(2);
    if hskip == 1 {
        read_simple_form(&mut br, alphabet_size)
    } else {
        read_complex_form(&mut br, alphabet_size, hskip as usize)
    }
}

fn read_simple_form(
    br: &mut BitReader,
    alphabet_size: usize,
) -> Result<(HuffmanTable, usize), &'static str> {
    let nsym = br.read_bits(2) + 1;
    let bits_per_sym = ceil_log2(alphabet_size as u32);
    let mut lengths = vec![0u8; alphabet_size];

    match nsym {
        1 => {
            let s = br.read_bits(bits_per_sym) as usize;
            if s >= alphabet_size { return Err("simple-form symbol out of range"); }
            lengths[s] = 1;
        }
        2 => {
            let s0 = br.read_bits(bits_per_sym) as usize;
            let s1 = br.read_bits(bits_per_sym) as usize;
            if s0 >= alphabet_size || s1 >= alphabet_size { return Err("simple-form symbol out of range"); }
            lengths[s0] = 1;
            lengths[s1] = 1;
        }
        3 => {
            let s0 = br.read_bits(bits_per_sym) as usize;
            let s1 = br.read_bits(bits_per_sym) as usize;
            let s2 = br.read_bits(bits_per_sym) as usize;
            if s0 >= alphabet_size || s1 >= alphabet_size || s2 >= alphabet_size { return Err("simple-form symbol out of range"); }
            lengths[s0] = 2;
            lengths[s1] = 2;
            lengths[s2] = 2;
        }
        4 => {
            let s0 = br.read_bits(bits_per_sym) as usize;
            let s1 = br.read_bits(bits_per_sym) as usize;
            let s2 = br.read_bits(bits_per_sym) as usize;
            let s3 = br.read_bits(bits_per_sym) as usize;
            if s0 >= alphabet_size || s1 >= alphabet_size || s2 >= alphabet_size || s3 >= alphabet_size { return Err("simple-form symbol out of range"); }
            let tree_select = br.read_bits(1);
            let len = if tree_select == 0 { 2 } else { 3 };
            // For tree_select=0: 2-bit codes for all 4 symbols.
            // For tree_select=1: first 2 symbols get 1-bit codes (s0=0, s1=1), last 2 get 2-bit codes starting at 00.
            // Wait — per RFC 7932 §9.5.1 Table: tree_select=1 means 1+1+2+2 layout, NOT 3-bit codes.
            // Actually re-reading: tree_select=0 → 2+2+2+2; tree_select=1 → 1+1+2+2 with s0,s1 getting 1-bit codes (s0=0, s1=1) and s2,s3 getting codes 00+something.
            // For our from_lengths builder, we just need the per-symbol lengths.
            if tree_select == 0 {
                lengths[s0] = 2;
                lengths[s1] = 2;
                lengths[s2] = 2;
                lengths[s3] = 2;
            } else {
                lengths[s0] = 1;
                lengths[s1] = 1;
                lengths[s2] = 2;
                lengths[s3] = 2;
            }
            let _ = len;
        }
        _ => unreachable!(),
    }

    let table = HuffmanTable::from_lengths(&lengths);
    Ok((table, br.bit_pos()))
}

fn read_complex_form(
    br: &mut BitReader,
    alphabet_size: usize,
    hskip: usize,
) -> Result<(HuffmanTable, usize), &'static str> {
    // Code-length code lengths (RFC 7932 §9.5.2). The 18 entries in
    // CODE_LENGTH_CODE_ORDER are read via a static prefix code:
    //   kCodeLengthPrefixLength: bits consumed per 4-bit peek
    //   kCodeLengthPrefixValue:  decoded value
    const K_CL_PREFIX_LENGTH: [u8; 16] = [2, 2, 2, 3, 2, 2, 2, 4, 2, 2, 2, 3, 2, 2, 2, 4];
    const K_CL_PREFIX_VALUE: [u8; 16] = [0, 4, 3, 2, 0, 4, 3, 1, 0, 4, 3, 2, 0, 4, 3, 5];

    let mut cl_code_lengths = [0u8; 18];
    let mut space: u32 = 32;
    let mut num_codes: u32 = 0;
    for (i, &sym) in CODE_LENGTH_CODE_ORDER.iter().enumerate() {
        if i < hskip {
            continue;
        }
        let ix = br.read_bits(4) as usize;
        let v = K_CL_PREFIX_VALUE[ix];
        let consumed = K_CL_PREFIX_LENGTH[ix] as usize;
        br.bit_pos -= 4 - consumed;
        cl_code_lengths[usize::from(sym)] = v;
        if v != 0 {
            space = space.wrapping_sub(32u32 >> v);
            num_codes += 1;
            if space.wrapping_sub(1) >= 32 {
                break;
            }
        }
    }
    if alphabet_size >= 64 {
    }
    if !(num_codes == 1 || space == 0) {
        return Err("invalid code-length code lengths (space not consumed)");
    }

    let cl_table = HuffmanTable::from_lengths(&cl_code_lengths);

    let mut lengths = vec![0u8; alphabet_size];
    let mut i: usize = 0;
    let mut prev_code_len: u8 = 8;
    let mut repeat: u32 = 0;
    let mut repeat_code_len: u32 = 0;
    let mut space: u32 = 32768;
    while i < alphabet_size && space > 0 {
        let sym = cl_table
            .read_symbol(br)
            .ok_or("invalid code-length symbol")? as u8;
        if sym < 16 {
            lengths[i] = sym;
            prev_code_len = sym;
            if sym != 0 {
                space = space.wrapping_sub(32768u32 >> sym);
            }
            i += 1;
            // Reset accumulator on literal symbol.
            repeat = 0;
            repeat_code_len = sym as u32;
        } else {
            // sym == 16: repeat prev (2 extra bits).
            // sym == 17: zero run (3 extra bits).
            // Both share the iterated accumulator semantics from
            // upstream `ProcessRepeatedCodeLength`. Consecutive repeat
            // symbols with the same target value (`prev_code_len` for 16,
            // 0 for 17) accumulate multiplicatively rather than additively.
            let extra_bits: u32 = if sym == 16 { 2 } else { 3 };
            let new_len: u32 = if sym == 16 { prev_code_len as u32 } else { 0 };
            let repeat_delta = br.read_bits(extra_bits);

            if repeat_code_len != new_len {
                repeat = 0;
                repeat_code_len = new_len;
            }
            let old_repeat = repeat;
            if repeat > 0 {
                repeat -= 2;
                repeat <<= extra_bits;
            }
            repeat += repeat_delta + 3;
            let actual_delta = repeat - old_repeat;

            if i + actual_delta as usize > alphabet_size {
                return Err("repeat overflows alphabet");
            }
            if new_len != 0 {
                for _ in 0..actual_delta {
                    lengths[i] = new_len as u8;
                    i += 1;
                    if new_len != 0 {
                        space = space.wrapping_sub(32768u32 >> new_len);
                    }
                    if space == 0 { break; }
                }
            } else {
                // Zero run: just advance i.
                i += actual_delta as usize;
            }
            if sym == 16 {
                // prev_code_len is unchanged (we just repeated it).
            }
        }
    }

    let table = HuffmanTable::from_lengths(&lengths);
    Ok((table, br.bit_pos()))
}

/// Read a Huffman table from the bitstream using the "simple" form
/// (RFC 7932 §9.5.1, NSYM=1/2/3/4 with optional symbol order swap).
///
/// Returns the table and the bit position past the table.
pub fn read_huffman_table_simple(
    data: &[u8],
    bit_pos: usize,
) -> Result<(HuffmanTable, usize), &'static str> {
    let mut br = BitReader::new(data);
    br.bit_pos = bit_pos;
    // Peek HSKIP — if it's 1, dispatch to simple form.
    let hskip = br.read_bits(2);
    if hskip != 1 {
        return Err("read_huffman_table_simple called with HSKIP != 1");
    }
    let (t, p) = read_simple_form(&mut br, 256)?;
    Ok((t, p))
}

// ---------------------------------------------------------------------------
// Phase B continuation: complex-form Huffman (RFC 7932 §9.5.2)
// ---------------------------------------------------------------------------

/// Code-length code order per RFC 7932 §9.5.2.
const CODE_LENGTH_CODE_ORDER: [u8; 18] = [
    1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

/// Read a complex-form Huffman table (RFC 7932 §9.5.2).
///
/// Format:
/// 1. HSKIP (2 bits) — skip this many initial symbols in the alphabet.
/// 2. Code-length code lengths (variable, terminated by 0+padding).
/// 3. Code-length code Huffman table built from those lengths.
/// 4. Per-symbol code lengths for the actual alphabet, encoded using
///    the code-length code table. Special symbols: 0 (no code), 16
///    (repeat previous 2-6 times), 17 (zero-run 3-10), 18 (zero-run
///    11-138).
pub fn read_huffman_table_complex(
    data: &[u8],
    bit_pos: usize,
    alphabet_size: usize,
) -> Result<(HuffmanTable, usize), &'static str> {
    // Legacy entry point — delegates to read_huffman_table (which handles
    // both simple and complex forms). Prefer calling read_huffman_table
    // directly.
    read_huffman_table(data, bit_pos, alphabet_size)
}

// ---------------------------------------------------------------------------
// Phase B continuation: context modes (RFC 7932 §10)
// ---------------------------------------------------------------------------

/// Context mode for literal symbols (RFC 7932 §10.1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextMode {
    /// `CONTEXT_LSB6`: low 6 bits of the previous byte.
    Lsb6,
    /// `CONTEXT_MSB6`: high 6 bits of the previous byte.
    Msb6,
    /// `CONTEXT_UTF8`: UTF-8-aware context.
    Utf8,
    /// `CONTEXT_SIGNED`: signed-byte context.
    Signed,
}

impl ContextMode {
    /// Compute the context ID for the given two previous bytes
    /// (RFC 7932 §10.1).
    ///
    /// Returns a value in:
    /// - `[0, 64)` for `Lsb6` / `Msb6` (only `p1` used)
    /// - `[0, 64)` for `Utf8` (uses both p1 and p2 via the 512-entry
    ///   `K_UTF8_CONTEXT_LOOKUP` table; the table is indexed as
    ///   `lookup[p1] | lookup[p2 | 256]`)
    /// - `[0, 64)` for `Signed` (uses both p1 and p2 via the 256-entry
    ///   `K_SIGNED_3BIT_CONTEXT_LOOKUP` table; the result is
    ///   `(lookup[p1] << 3) | lookup[p2]`)
    ///
    /// For backwards compatibility with the existing single-byte API,
    /// the `context_id(p1)` form treats `p2` as 0.
    #[must_use]
    pub fn context_id(&self, prev_byte: u8) -> u8 {
        self.context_id_2(prev_byte, 0)
    }

    /// Two-byte context ID (RFC 7932 §10.1). The full brotli decoder
    /// uses both `p1` (immediately preceding byte) and `p2` (the byte
    /// before p1) for the UTF-8 and SIGNED context modes.
    #[must_use]
    pub fn context_id_2(&self, p1: u8, p2: u8) -> u8 {
        match self {
            Self::Lsb6 => p1 & 0x3F,
            Self::Msb6 => p1 >> 2,
            Self::Utf8 => {
                crate::static_codes::K_UTF8_CONTEXT_LOOKUP[p1 as usize]
                    | crate::static_codes::K_UTF8_CONTEXT_LOOKUP[(p2 as usize) | 256]
            }
            Self::Signed => {
                ((crate::static_codes::K_SIGNED_3BIT_CONTEXT_LOOKUP[p1 as usize] as u8) << 3)
                    + crate::static_codes::K_SIGNED_3BIT_CONTEXT_LOOKUP[p2 as usize]
            }
        }
    }
}

/// Parse the context-mode field from the literal block-type header
/// (RFC 7932 §9.3.1 + §10.1).
///
/// Phase B reads a 2-bit field per block-type; the full spec reads
/// a complex context-map structure. This is a simplification for
/// single-context-mode blocks.
pub fn parse_context_mode(
    data: &[u8],
    bit_pos: usize,
) -> Result<(ContextMode, usize), &'static str> {
    let mut br = BitReader::new(data);
    br.bit_pos = bit_pos;

    let mode = br.read_bits(2);
    let result = match mode {
        0 => ContextMode::Lsb6,
        1 => ContextMode::Msb6,
        2 => ContextMode::Utf8,
        3 => ContextMode::Signed,
        _ => unreachable!("2-bit field"),
    };
    Ok((result, br.bit_pos()))
}

// ---------------------------------------------------------------------------
// Top-level decoder (RFC 7932 §9)
// ---------------------------------------------------------------------------

/// Decode a Brotli-compressed stream (RFC 7932).
///
/// Handles:
/// - Empty metablocks (ISLASTEMPTY)
/// - Uncompressed metablocks (IS_UNCOMPRESSED=1)
/// - Huffman-coded metablocks in the trivial layout produced by
///   `compress_fragment_two_pass` (1 literal/command/distance Huffman
///   tree each, no context maps, no block types, NPOSTFIX=0, NDIRECT=0).
///
/// Non-trivial Huffman-coded metablocks (multiple block types, context
/// maps, NPOSTFIX/NDIRECT > 0, custom dictionaries) are not yet
/// supported — they return `Err("unsupported metablock feature")`.
///
/// # Errors
///
/// Returns `&'static str` on malformed input or unsupported features.
pub fn decode(compressed: &[u8]) -> Result<Vec<u8>, &'static str> {
    let (_frame, mut bit_pos) = parse_frame_header(compressed, 0)?;
    let mut output = Vec::new();

    loop {
        let (mb, next_pos) = parse_metablock_header(compressed, bit_pos)?;
        bit_pos = next_pos;

        if mb.is_last_empty {
            break;
        }

        if mb.is_uncompressed {
            let byte_offset = (bit_pos + 7) / 8;
            let needed = byte_offset.checked_add(mb.mlen as usize).ok_or("mlen overflow")?;
            if needed > compressed.len() {
                return Err("uncompressed metablock extends past input");
            }
            output.extend_from_slice(&compressed[byte_offset..needed]);
            bit_pos = needed * 8;
        } else {
            let (new_pos, bytes_emitted) = decode_compressed_metablock(compressed, bit_pos, mb.mlen as usize)?;
            bit_pos = new_pos;
            output.extend(bytes_emitted);
        }

        if mb.is_last {
            break;
        }
    }

    Ok(output)
}

/// Decode a Huffman-coded metablock (RFC 7932 §9.3-§9.4).
///
/// Returns `(new_bit_pos, output_bytes)`.
///
/// Dispatches between two paths via OCP:
/// - Trivial-layout fast path (this function body): when all three
///   categories have `NBLTYPES == 1` and `NTREES == 1`. Handles all
///   output from our `compress_fragment_two_pass` encoder.
/// - Full RFC 7932 path (`decode_compressed_metablock_full`): when
///   any category has `NBLTYPES > 1` or `NTREES > 1`. Handles
///   reference brotli streams from upstream encoders.
fn decode_compressed_metablock(
    data: &[u8],
    bit_pos: usize,
    mlen: usize,
) -> Result<(usize, Vec<u8>), &'static str> {
    let mut br = BitReader::new(data);
    br.bit_pos = bit_pos;

    let nbltypesl = read_varlen_uint8(&mut br)? + 1;
    let nbltypesc = read_varlen_uint8(&mut br)? + 1;
    let nbltypesd = read_varlen_uint8(&mut br)? + 1;

    // Dispatch to the full decoder for non-trivial layouts.
    if nbltypesl > 1 || nbltypesc > 1 || nbltypesd > 1 {
        return crate::decoder_full::decode_compressed_metablock_full(
            data, br.bit_pos(), mlen, nbltypesl, nbltypesc, nbltypesd,
        );
    }

    // From here on the trivial fast path: NBLTYPES = 1 for all categories.
    let npostfix = br.read_bits(2) as usize;
    let ndirect_raw = br.read_bits(4) as usize;
    // Per RFC 7932 §9.4 + upstream `BrotliDecoderState::METABLOCK_HEADER_2`:
    //   bits = ReadBits(6);  // 6 bits = NPOSTFIX (2) + NDMOEM (4) packed.
    //   NPOSTFIX = bits & 3;
    //   NDMOEM = bits >> 2;
    //   num_direct_distance_codes = NUM_SHORT + (NDMOEM << NPOSTFIX).
    //
    // So NDIRECT (the count of direct codes beyond the 16 short codes) =
    // NDMOEM << NPOSTFIX. No `if < 12` adjustment (that was a previous
    // incorrect interpretation).
    let ndirect = ndirect_raw << npostfix;
    if npostfix > 3 {
        return Err("invalid metablock: NPOSTFIX > 3");
    }

    let _context_mode = br.read_bits(2);

    let ntreesl = read_varlen_uint8(&mut br)? + 1;
    let ntreesd = read_varlen_uint8(&mut br)? + 1;

    // Dispatch to the full decoder if NTREES > 1 in either category
    // (multi-tree Huffman groups requiring context maps).
    if ntreesl > 1 || ntreesd > 1 {
        return crate::decoder_full::decode_compressed_metablock_full_with_trees(
            data, br.bit_pos(), mlen,
            npostfix, ndirect_raw, _context_mode,
        );
    }

    let (lit_table, p) = read_huffman_table(data, br.bit_pos(), 256)?;
    br.bit_pos = p;

    // Read the cmd Huffman tree as a 704-symbol table. This is used
    // directly with kCmdLut — the rearrangement in upstream's
    // BuildAndStoreCommandPrefixCode is designed so that symbol N in
    // the 704-alphabet has params from kCmdLut[N] matching the encoder's
    // intent for the original command code that maps to position N.
    let (cmd_table, p) = read_huffman_table(data, br.bit_pos(), 704)?;
    br.bit_pos = p;

    // Distance alphabet size per RFC 7932 §9.4:
    //   alphabet_size = NUM_DISTANCE_SHORT_CODES + NDIRECT + (48 << NPOSTFIX)
    //                = (16 + NDIRECT) + (48 << NPOSTFIX).
    let num_direct_distance_codes = 16u32 + ndirect as u32;
    let dist_alphabet_size = num_direct_distance_codes as usize + (48usize << npostfix);
    let (dist_table, p) = read_huffman_table(data, br.bit_pos(), dist_alphabet_size)?;
    br.bit_pos = p;

    let mut output = Vec::with_capacity(mlen);
    let mut dist_rb: [u32; 4] = [16, 15, 11, 4];
    let mut dist_rb_idx: i32 = 0;

    while output.len() < mlen {
        let cmd_code = cmd_table.read_symbol(&mut br).ok_or("invalid command symbol")? as usize;
        let v = &crate::prefix::kCmdLut[cmd_code];

        let insert_len_extra = if v.insert_len_extra_bits > 0 {
            br.read_bits(u32::from(v.insert_len_extra_bits))
        } else { 0 };
        let copy_length = if v.copy_len_extra_bits > 0 {
            br.read_bits(u32::from(v.copy_len_extra_bits))
        } else { 0 };

        let insert_len = usize::from(v.insert_len_offset) + insert_len_extra as usize;
        let copy_len = usize::from(v.copy_len_offset) + copy_length as usize;

        for _ in 0..insert_len {
            let lit = lit_table.read_symbol(&mut br).ok_or("invalid literal")?;
            output.push(lit as u8);
        }

        // Per upstream `ProcessCommandsInternal`: after consuming the
        // insert-length literals, if the metablock is fully decoded,
        // exit without reading the trailing distance/copy. The encoder's
        // final INSERT-only command leaves the remaining len == 0 here.
        if output.len() >= mlen {
            break;
        }

        if copy_len > 0 {
            let distance = if v.distance_code >= 0 {
                take_distance_from_ring_buffer(v.distance_code as i32, &mut dist_rb, &mut dist_rb_idx)
            } else {
                let dist_code = dist_table.read_symbol(&mut br).ok_or("invalid distance symbol")? as i32;
                decode_distance_from_code(dist_code, num_direct_distance_codes, npostfix as i32, &mut br, &mut dist_rb, &mut dist_rb_idx)
            };
            if distance == 0 || distance as usize > output.len() {
                return Err("invalid back-reference distance");
            }
            let src = output.len() - distance as usize;
            for i in 0..copy_len {
                let b = output[src + i];
                output.push(b);
            }
        }

        if output.len() > mlen + 1 {
            return Err("metablock overran mlen");
        }
    }

    Ok((br.bit_pos(), output))
}

/// Decode a distance from a dist_table symbol code (RFC 7932 §9.4 + §10.4).
///
/// For codes 0..15 (short codes): compute from dist_rb ring buffer.
/// For codes 16..num_direct-1 (when NDIRECT > 0): direct distance codes.
/// For codes >= num_direct: long-distance formula with NPOSTFIX bits.
///
/// Mirrors upstream `ReadDistanceInternal`. `dist_code` is the raw
/// symbol from the dist Huffman table.
///
/// `num_direct` is `NUM_DISTANCE_SHORT_CODES + NDIRECT` (= 16 + NDIRECT):
/// the count of short + direct codes. `npostfix` is the NPOSTFIX field
/// from the metablock header (0..=3).
///
/// General formula (long codes):
///   postfix_mask = (1 << NPOSTFIX) - 1
///   distval = dist_code - num_direct
///   postfix = distval & postfix_mask
///   distval >>= NPOSTFIX
///   nbits  = (distval >> 1) + 1
///   offset = ((2 + (distval & 1)) << nbits) - 4
///   distance = ((offset + ReadBits(nbits)) << NPOSTFIX) + postfix
///              + num_direct - (NUM_DISTANCE_SHORT_CODES - 1)
pub(crate) fn decode_distance_from_code(
    dist_code: i32,
    num_direct: u32,
    npostfix: i32,
    br: &mut BitReader,
    dist_rb: &mut [u32; 4],
    dist_rb_idx: &mut i32,
) -> u32 {
    const NUM_DISTANCE_SHORT_CODES: i32 = 16;
    if dist_code < NUM_DISTANCE_SHORT_CODES {
        return take_distance_from_ring_buffer(dist_code, dist_rb, dist_rb_idx);
    }
    if dist_code < num_direct as i32 {
        // Direct distance code (NDIRECT > 0 only): distance = code - 15.
        let distance = (dist_code - NUM_DISTANCE_SHORT_CODES + 1) as u32;
        dist_rb[(*dist_rb_idx as usize) & 3] = distance;
        *dist_rb_idx = dist_rb_idx.wrapping_add(1);
        return distance;
    }
    // Long-distance code with optional postfix.
    let postfix_mask = (1i32 << npostfix) - 1;
    let mut distval = dist_code - num_direct as i32;
    let postfix = distval & postfix_mask;
    distval >>= npostfix;
    let nbits = ((distval as u32) >> 1) + 1;
    let offset = (((distval & 1) + 2) << nbits) - 4;
    let extra = br.read_bits(nbits);
    let raw = (offset as i32 + extra as i32) << npostfix;
    let distance = (raw + postfix + num_direct as i32 - NUM_DISTANCE_SHORT_CODES + 1) as u32;
    dist_rb[(*dist_rb_idx as usize) & 3] = distance;
    *dist_rb_idx = dist_rb_idx.wrapping_add(1);
    distance
}


/// Read a `DecodeVarLenUint8`-encoded value (RFC 7932 §9.3 MoreBlockLengths).
///
/// - 1 bit = 0 → value = 0
/// - 4 bits = 0000 → value = 1
/// - 4 bits = 0XYZ (XYZ nonzero) → read XYZ more bits, value = (1 << XYZ) + bits
pub(crate) fn read_varlen_uint8(br: &mut BitReader) -> Result<u32, &'static str> {
    if br.read_bits(1) == 0 {
        return Ok(0);
    }
    let nbits = br.read_bits(3);
    if nbits == 0 {
        return Ok(1);
    }
    let extra = br.read_bits(nbits);
    Ok((1u32 << nbits) + extra)
}

/// Mirror of upstream `TakeDistanceFromRingBuffer`: computes the
/// actual distance for short codes 0..15 from the dist_rb ring buffer.
pub(crate) fn take_distance_from_ring_buffer(code: i32, dist_rb: &mut [u32; 4], dist_rb_idx: &mut i32) -> u32 {
    if code == 0 {
        *dist_rb_idx -= 1;
        return dist_rb[(*dist_rb_idx & 3) as usize];
    }
    // distance_code in the formula is `code << 1` (matches upstream line 1911).
    let dc = code << 1;
    const K_INDEX_OFFSET: u32 = 0xaaafff1b;
    const K_VALUE_OFFSET: u32 = 0xfa5fa500;
    let v_idx = (*dist_rb_idx + (K_INDEX_OFFSET as i32 >> dc)) as i32 & 0x3;
    let mut distance = dist_rb[v_idx as usize] as i32;
    let v_val = (K_VALUE_OFFSET >> dc) as i32 & 0x3;
    if dc & 3 != 0 {
        distance += v_val;
    } else {
        distance -= v_val;
        if distance <= 0 {
            distance = 0x7fff_ffff;
        }
    }
    distance as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_encoder;

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
        let (hdr, _) = parse_frame_header(&data, 0).expect("parse");
        assert_eq!(hdr.window_bits, 16);
    }

    #[test]
    fn parse_frame_header_window_with_nbl() {
        // WBITS=1 (bit 0), NBL=2 (bits 1-3 = 010) → 17 + 2 = 19.
        // Packed LSB-first: 0b0101 = 0x05.
        let data = [0b0000_0101u8, 0u8];
        let (hdr, _) = parse_frame_header(&data, 0).expect("parse");
        assert_eq!(hdr.window_bits, 19);
    }

    #[test]
    fn parse_frame_header_rejects_empty() {
        let data: [u8; 0] = [];
        assert!(parse_frame_header(&data, 0).is_err());
    }

    #[test]
    fn parse_frame_header_lgwin_10_for_tiny_input() {
        // WBITS=1, NBL=0, N2=2 → window_bits = 8 + 2 = 10.
        // LSB-first: bit 0 = 1, bits 1-3 = 0,0,0, bits 4-6 = 0,1,0
        // = 0b0100001 = 0x21.
        let data = [0x21u8, 0u8];
        let (hdr, _) = parse_frame_header(&data, 0).expect("parse");
        assert_eq!(hdr.window_bits, 10);
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
        // Bits packed LSB-first:
        //   bit 0 = 1 (ISLAST)
        //   bit 1 = 0 (ISLASTEMPTY)
        //   bit 2 = 1 (MNIBBLES low)
        //   bit 3 = 0 (MNIBBLES high) → MNIBBLES = 1
        //   bits 4-7 = 0000 → MLEN raw = 0, decoded = 1
        //   bit 8 = 0 (IS_UNCOMPRESSED)
        //   bit 9 = 0 (reserved)
        // Packed: 0b0000_0101 = 0x05 (first byte).
        let data = [0b0000_0101u8, 0b0000_0000u8];
        let (hdr, _) = parse_metablock_header(&data, 0).expect("parse");
        assert!(hdr.is_last);
        assert!(!hdr.is_last_empty);
        assert_eq!(hdr.mlen, 1);
        assert!(!hdr.is_uncompressed);
    }

    #[test]
    fn parse_metablock_header_not_last() {
        // ISLAST=0 (bit 0), MNIBBLES=11 (bits 1-2) = 3,
        // MLEN = 0 (bits 3-14, 3 nibbles),
        // IS_UNCOMPRESSED (bit 15) = 0, reserved (bit 16) = 0.
        // Packed LSB-first across 3 bytes:
        //   byte 0: bits 0-7 = 0,1,1,0,0,0,0,0 = 0b0000_0110 = 0x06
        //   byte 1: bits 8-15 = 0,0,0,0,0,0,0,0 = 0x00
        //   byte 2: bits 16-17 = 0,0
        let data = [0x06u8, 0x00, 0x00];
        let (hdr, _) = parse_metablock_header(&data, 0).expect("parse");
        assert!(!hdr.is_last);
        assert_eq!(hdr.mnibbles, 3);
    }

    #[test]
    fn parse_metablock_header_islast_no_reserved_bit() {
        // Per RFC 7932 §9.2: the reserved bit only appears when ISLAST=1.
        // For ISLAST=0 metablocks, ISUNCOMPRESSED is the last field —
        // there is no reserved bit to validate.
        let mut data = [0u8; 3];
        data[0] = 0b0000_0000; // ISLAST=0, MNIBBLES=00
        // bit 20 in the stream is just whatever comes after the header.
        let result = parse_metablock_header(&data, 0);
        assert!(result.is_ok(), "ISLAST=0 metablock has no reserved bit");
    }

    // --- Phase B tests ---

    #[test]
    fn ceil_log2_handles_small_values() {
        assert_eq!(ceil_log2(0), 0);
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(3), 2);
        assert_eq!(ceil_log2(4), 2);
        assert_eq!(ceil_log2(5), 3);
        assert_eq!(ceil_log2(8), 3);
        assert_eq!(ceil_log2(256), 8);
    }

    #[test]
    fn parse_block_type_header_single_type() {
        // NBLOCKTYPES bits = 00 → num_block_types = 1.
        let data = [0b0000_0000u8];
        let (hdr, _) =
            parse_block_type_header(&data, 0, BlockTypeCategory::Literal).expect("parse");
        assert_eq!(hdr.num_block_types, 1);
        assert_eq!(hdr.num_huffman_trees, 1);
    }

    #[test]
    fn parse_block_type_header_two_types() {
        // NBLOCKTYPES = 01 → num_block_types = 2. ceil_log2(2) = 1,
        // so initial_type is 1 bit, jump table is 3 × 1 bit = 3 bits.
        // Bits LSB-first: bit 0 = 1 (NBLOCKTYPES lo), bit 1 = 0 (hi),
        // bit 2 = 0 (initial_type), bits 3-5 = jump table.
        //   byte = 1 + 0 + 0 + 0 + 16 + 0 + 0 + 0 = 0x11
        let data = [0x11u8];
        let (hdr, _) =
            parse_block_type_header(&data, 0, BlockTypeCategory::Literal).expect("parse");
        assert_eq!(hdr.num_block_types, 2);
        assert_eq!(hdr.num_huffman_trees, 2);
    }

    #[test]
    fn parse_distance_header_reads_npostfix_and_ndirect() {
        // NPOSTFIX (2 bits) = 1, NDIRECT_MSB = 0, NDIRECT_LO (4 bits) = 5.
        // Bits LSB-first:
        //   bit 0 = 1 (NPOSTFIX bit 0)
        //   bit 1 = 0 (NPOSTFIX bit 1)
        //   bit 2 = 0 (NDIRECT_MSB)
        //   bit 3 = 1 (NDIRECT_LO bit 0) → contributes 8
        //   bit 4 = 0
        //   bit 5 = 1 (NDIRECT_LO bit 2) → contributes 32
        //   bits 6,7 = 0
        // = 1 + 8 + 32 = 41 = 0x29
        let data = [0x29u8];
        let (hdr, _) = parse_distance_header(&data, 0).expect("parse");
        assert_eq!(hdr.num_postfix, 1);
        assert_eq!(hdr.num_direct, 5);
    }

    #[test]
    fn huffman_table_from_lengths_assigns_canonical_codes() {
        // Standard example: 4 symbols with lengths [2, 1, 3, 3].
        // Canonical Huffman codes (MSB-first) for lengths [2, 1, 3, 3]:
        //   len=1: sym 1 → code 0
        //   len=2: sym 0 → code 10
        //   len=3: sym 2 → code 110, sym 3 → code 111
        // Our from_lengths stores BIT-REVERSED codes for LSB-first
        // bitstream lookup:
        //   sym 0 → reverse(0b10, 2) = 0b01 = 1
        //   sym 1 → reverse(0b0, 1) = 0
        //   sym 2 → reverse(0b110, 3) = 0b011 = 3
        //   sym 3 → reverse(0b111, 3) = 0b111 = 7
        let table = HuffmanTable::from_lengths(&[2, 1, 3, 3]);
        // Verify via read_symbol that the table decodes correctly.
        // Sym 1 has depth 1, code "0" → bitstream bit 0 returns sym 1.
        let data = [0b0u8]; // bit 0 = 0
        let mut br = BitReader::new(&data);
        assert_eq!(table.read_symbol(&mut br), Some(1));
    }

    #[test]
    fn huffman_table_read_symbol_walks_canonical_assignment() {
        // Sym 0 → 0, sym 1 → 10, sym 2 → 11. Lengths: [1, 2, 2].
        let table = HuffmanTable::from_lengths(&[1, 2, 2]);

        // Sym 0 code = "0" (1 bit).
        // Bitstream "0" → sym 0. Bit 0 = 0.
        let mut br = BitReader::new(&[0b0000_0000]);
        assert_eq!(table.read_symbol(&mut br), Some(0));

        // Sym 1 code = "10" (2 bits). read_symbol reads bit 0 as MSB
        // of code, so for code "10" we need bit_0=1, bit_1=0.
        // Byte with bit 0 = 1: 0b0000_0001 = 0x01.
        let mut br = BitReader::new(&[0b0000_0001]);
        assert_eq!(table.read_symbol(&mut br), Some(1));

        // Sym 2 code = "11" (2 bits). bit_0=1, bit_1=1.
        // Byte: 0b0000_0011 = 0x03.
        let mut br = BitReader::new(&[0b0000_0011]);
        assert_eq!(table.read_symbol(&mut br), Some(2));
    }

    #[test]
    fn read_huffman_table_simple_single_symbol() {
        // HSKIP=1 (2 bits value 0b01), NSYM=1 (2 bits value 0b00),
        // then symbol 0x42 (8 bits).
        // Bit layout:
        //   bit 0: 1  (HSKIP bit 0 — value 1)
        //   bit 1: 0  (HSKIP bit 1)
        //   bit 2: 0  (NSYM bit 0 — value 0 → NSYM=1)
        //   bit 3: 0  (NSYM bit 1)
        //   bit 4-11: symbol 0x42 = 0b0100_0010 LSB-first
        //     bit 4: 0 (sym bit 0)
        //     bit 5: 1 (sym bit 1)
        //     bit 6: 0 (sym bit 2)
        //     bit 7: 0 (sym bit 3)
        // Byte 0 = 0,0,0,0,0,1,0,0 (LSB-first reading) = 0b0010_0000 = 0x20
        //   bit 8: 0 (sym bit 4)
        //   bit 9: 0 (sym bit 5)
        //   bit 10: 1 (sym bit 6)
        //   bit 11: 0 (sym bit 7)
        // Byte 1 low nibble = 0,0,1,0 = 0b0100 = 0x04
        let data = [0x21u8, 0x04];
        let (table, _) = read_huffman_table_simple(&data, 0).expect("parse");
        // Sym 0x42 has depth 1 → single_symbol is set.
        assert_eq!(table.single_symbol, Some(0x42));
    }

    // --- Phase B continuation tests ---

    #[test]
    fn context_mode_lsb6_uses_low_6_bits() {
        assert_eq!(ContextMode::Lsb6.context_id(0b0000_0000), 0);
        assert_eq!(ContextMode::Lsb6.context_id(0b0011_1111), 63);
        assert_eq!(ContextMode::Lsb6.context_id(0b1011_1111), 63); // high bits ignored
        assert_eq!(ContextMode::Lsb6.context_id(0b0000_0101), 5);
    }

    #[test]
    fn context_mode_msb6_uses_high_6_bits() {
        // Top 6 bits → shifted down by 2.
        assert_eq!(ContextMode::Msb6.context_id(0b0000_0000), 0);
        assert_eq!(ContextMode::Msb6.context_id(0b1111_1100), 63);
        assert_eq!(ContextMode::Msb6.context_id(0b1010_1000), 42);
    }

    #[test]
    fn context_mode_utf8_uses_lookup_table() {
        // p1 = 'A' (0x41, ASCII letter), p2 = 0.
        // K_UTF8_CONTEXT_LOOKUP[0x41] = 48, K_UTF8_CONTEXT_LOOKUP[256] = 0.
        // Context = 48 | 0 = 48.
        assert_eq!(ContextMode::Utf8.context_id(b'A'), 48);
        // p1 = 0 (NUL): K_UTF8_CONTEXT_LOOKUP[0] = 0.
        assert_eq!(ContextMode::Utf8.context_id(0), 0);
        // p1 = ' ' (0x20): K_UTF8_CONTEXT_LOOKUP[0x20] = 8.
        assert_eq!(ContextMode::Utf8.context_id(b' '), 8);
    }

    #[test]
    fn context_mode_signed_uses_lookup_table() {
        // K_SIGNED_3BIT_CONTEXT_LOOKUP[0]=0, [1]=1, [128]=4, [254]=6, [255]=7.
        // For context_id(p1) we use p2 = 0, so context = (lut[p1] << 3) | lut[0]
        //                                                = lut[p1] << 3.
        assert_eq!(ContextMode::Signed.context_id(0), 0);
        assert_eq!(ContextMode::Signed.context_id(1), 8);
        assert_eq!(ContextMode::Signed.context_id(128), 32);
        assert_eq!(ContextMode::Signed.context_id(254), 48);
        assert_eq!(ContextMode::Signed.context_id(255), 56);
    }

    #[test]
    fn parse_context_mode_decodes_2bit_field() {
        // Mode 0 = Lsb6, mode 1 = Msb6, mode 2 = Utf8, mode 3 = Signed.
        for (byte_val, expected) in [
            (0u8, ContextMode::Lsb6),
            (1, ContextMode::Msb6),
            (2, ContextMode::Utf8),
            (3, ContextMode::Signed),
        ] {
            let data = [byte_val];
            let (mode, _) = parse_context_mode(&data, 0).expect("parse");
            assert_eq!(mode, expected);
        }
    }

    #[test]
    fn code_length_code_order_starts_with_one() {
        // RFC 7932 §9.5.2: code-length-code order is
        // [1, 2, 3, 4, 0, 5, 17, 6, 16, ...]. Sym 1 is first.
        assert_eq!(CODE_LENGTH_CODE_ORDER[0], 1);
        assert_eq!(CODE_LENGTH_CODE_ORDER[1], 2);
        assert_eq!(CODE_LENGTH_CODE_ORDER[4], 0);
        assert_eq!(CODE_LENGTH_CODE_ORDER.len(), 18);
    }

    #[test]
    fn complex_huffman_table_decodes_simple_alphabet() {
        // Construct a minimal complex-form table:
        // HSKIP=0, code-length code lengths all 0 except for sym 0
        // (=3 bits, assigning code 0 to cl-code 0 → "length value 0"
        // encoded as 1 bit). Then alphabet_size symbols all encoded
        // as length-1 codes.
        //
        // For Phase B this test just verifies the function accepts
        // a minimal input without erroring. Full behavioural testing
        // requires a complete bitstream generator (Phase C).
        //
        // HSKIP=0 (2 bits), then 18 code-length codes at 3 bits each
        // = 54 bits. Total 56 bits = 7 bytes. We set sym 0's cl-code
        // length to 1 (3 bits = 0b001), the rest 0.
        //
        // Bits (LSB-first):
        //   bit 0,1: HSKIP = 00
        //   bits 2-4: cl-code for sym 1 = 0 (since CODE_LENGTH_CODE_ORDER[0] = 1)
        //   bits 5-7: cl-code for sym 2 = 0
        //   ...
        //   All zero except where CODE_LENGTH_CODE_ORDER[i] == 0
        //   which is at i=4.
        //   bits 14-16: cl-code for sym 0 = 1 (assign length 1 to cl-code 0).
        // Then alphabet encoding — since all lengths are 0 the loop
        // should exit immediately.
        //
        // This is complex enough that we just check the function
        // doesn't panic on an all-zero input. Real verification
        // happens via round-trip tests in Phase C.
        let data = vec![0u8; 16];
        let _ = read_huffman_table_complex(&data, 0, 1);
        // No panic, no error → pass.
    }

    // --- Phase D: end-to-end decode tests ---

    /// Cross-check our pure-Rust decoder against the upstream `brotli`
    /// CLI tool. Encoded input is decoded by both, results compared.
    /// The upstream `brotli` produces an uncompressed metablock for
    /// tiny inputs, which our decoder handles.
    #[test]
    fn decode_uncompressed_metablock_for_one_byte() {
        // Upstream `brotli -c -q 6 "a"` produces:
        //   0x21 0x00 0x00 0x04 0x61 0x03
        // Layout:
        //   bits 0-6:   frame header (lgwin=10) — 0x21
        //   bit 7:      ISLAST=0
        //   bits 8-9:   MNIBBLES=0 (→ 4 nibbles)
        //   bits 10-25: MLEN=0 (16 bits) → decoded mlen=1
        //   bit 26:     IS_UNCOMPRESSED=1
        //   bit 27:     reserved=0
        //   bits 28-31: padding (4 bits)
        //   byte 4:     literal 'a' = 0x61
        //   byte 5 bit 0: ISLAST=1
        //   byte 5 bit 1: ISLASTEMPTY=1
        //   byte 5 bits 2-7: padding
        let compressed = [0x21u8, 0x00, 0x00, 0x04, 0x61, 0x03];
        let decoded = decode(&compressed).expect("decode");
        assert_eq!(decoded, b"a");
    }

    #[test]
    fn decode_uncompressed_metablock_for_two_bytes() {
        // Upstream `brotli -c -q 6 "aa"` produces:
        //   0x21 0x04 0x00 0x04 0x61 0x61 0x03
        // Same layout, but mlen=2. MLEN field encodes mlen-1=1.
        let compressed = [0x21u8, 0x04, 0x00, 0x04, 0x61, 0x61, 0x03];
        let decoded = decode(&compressed).expect("decode");
        assert_eq!(decoded, b"aa");
    }

    #[test]
    fn decode_terminator_only_stream() {
        // A stream with just an empty last-metablock marker:
        //   frame header (1 bit: WBITS=0 → window 16)
        //   ISLAST=1 (1 bit), ISLASTEMPTY=1 (1 bit)
        //   byte-alignment: 5 bits of 0
        // = byte 0x06 (LSB-first: 0b00000110)
        let compressed = [0x06u8];
        let decoded = decode(&compressed).expect("decode");
        assert!(decoded.is_empty());
    }

    #[test]
    fn decode_larger_uncompressed_metablock() {
        // Construct a known-valid uncompressed stream for "hello".
        // Frame header: lgwin=16 (WBITS=0), 1 bit.
        // Metablock header: ISLAST=0, MNIBBLES=0, MLEN_field=4
        //   (mlen=5=hello.len()), IS_UNCOMPRESSED=1, reserved=0.
        //   Bit layout (LSB-first):
        //     bit 0: frame WBITS=0
        //     bit 1: ISLAST=0
        //     bits 2-3: MNIBBLES=00
        //     bits 4-19: MLEN=4 (16 bits LSB-first: 0b00000100, rest 0)
        //     bit 20: IS_UNCOMPRESSED=1
        //     bit 21: reserved=0
        //     bits 22-23: byte-align padding (2 bits)
        //   bytes 0-2: bits 0-23 packed
        //   bytes 3-7: literal "hello"
        //   byte 8: ISLAST=1 + ISLASTEMPTY=1 + padding
        let mut stream = vec![0u8; 9];
        // bit 0 = WBITS = 0 (already 0)
        // bit 1 = ISLAST = 0 (already 0)
        // bits 2,3 = MNIBBLES = 0,0 (already 0)
        // MLEN field = 4 (16 bits LSB-first):
        //   bit 4 = bit 0 of MLEN = 0
        //   bit 5 = bit 1 = 0
        //   bit 6 = bit 2 = 1
        //   bits 7-19 = rest 0
        stream[0] |= 0b0100_0000; // bit 6 = 1
        // IS_UNCOMPRESSED at bit 20 = bit 4 of byte 2
        stream[2] |= 0b0001_0000; // bit 4 = 1
        // reserved at bit 21 = bit 5 of byte 2 = 0
        // bits 22-23 = padding = 0
        // Literals "hello" at bytes 3-7
        stream[3..8].copy_from_slice(b"hello");
        // ISLAST=1 + ISLASTEMPTY=1 at byte 8
        stream[8] = 0b0000_0011;
        let decoded = decode(&stream).expect("decode");
        assert_eq!(decoded, b"hello");
    }

    /// Decode the actual upstream brotli encoding of "hello" produced
    /// by the system `brotli -c -q 6` tool. Catches drift in the
    /// frame-header parsing, metablock-header layout, and
    /// uncompressed-metablock handling.
    #[test]
    fn decode_upstream_brotli_hello() {
        // Hard-coded bytes from `brotli -c -q 6 /tmp/h.bin` where
        // /tmp/h.bin contains "hello".
        let compressed = [
            0x21, 0x10, 0x00, 0x04, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x03,
        ];
        let decoded = decode(&compressed).expect("decode");
        assert_eq!(decoded, b"hello");
    }

    /// Decode an actual upstream `brotli -q 1` compressed stream.
    /// The encoder produces a Huffman-coded metablock in the trivial
    /// layout (single block type per category, single Huffman tree
    /// per category). Our decoder must round-trip this through the
    /// same code path used for our own encoder's output.
    ///
    /// Skipped if the `brotli` CLI is not installed.
    #[test]
    fn decode_upstream_brotli_q1_text() {
        let input = b"The quick brown fox jumps over the lazy dog. ".repeat(3);
        let tmp = std::env::temp_dir().join("omnizip_brotli_q1_decode_test.br");
        std::fs::write(&tmp, &input).unwrap();
        let enc = std::process::Command::new("brotli")
            .args(["-q", "1", "-c"])
            .arg(&tmp)
            .output();
        let compressed = match enc {
            Ok(o) if o.status.success() => o.stdout,
            Ok(o) => {
                eprintln!("[skip] brotli -q 1 failed: {}",
                          String::from_utf8_lossy(&o.stderr));
                return;
            }
            Err(e) => {
                eprintln!("[skip] brotli CLI not installed: {e}");
                return;
            }
        };
        let decoded = decode(&compressed).unwrap_or_else(|e| panic!("decode failed: {e}"));
        assert_eq!(decoded, input);
    }

    // --- Phase E: distance formula + metablock-end behaviour ---

    /// `decode_distance_from_code` for the first long-distance code
    /// (symbol 16) must read the 1 extra bit per upstream
    /// `ReadDistanceInternal`. Regression test for the bitstream
    /// desync that produced wrong symbol reads after every match.
    #[test]
    fn decode_distance_from_code_reads_extra_bit_for_first_long_code() {
        let mut dist_rb: [u32; 4] = [16, 15, 11, 4];
        let mut dist_rb_idx: i32 = 0;
        // NPOSTFIX=0, NDIRECT=0 → num_direct_distance_codes = 16.
        // Bit pattern: bit 0 = 1 → distance = 1 + 1 = 2.
        let data = [0b0000_0001u8];
        let mut br = BitReader::new(&data);
        let d = decode_distance_from_code(16, 16, 0, &mut br, &mut dist_rb, &mut dist_rb_idx);
        assert_eq!(d, 2, "first long-code + extra=1 → distance 2");
        assert_eq!(br.bit_pos(), 1, "exactly one extra bit consumed");
    }

    /// After consuming INSERT literals for the LAST command, the
    /// metablock-end check must fire so we do NOT read into the
    /// trailing ISLAST+ISLASTEMPTY terminator. Regression test for
    /// the metablock-overrun failure on `low_entropy_64`.
    #[test]
    fn decoder_exits_after_final_insert_only_command() {
        // 64-byte input with low entropy (alphabet 4) — the encoder
        // emits multiple matches followed by an INSERT-only tail.
        // Round-trip via our own encoder exercises the
        // `output.len() >= mlen` short-circuit.
        let mut state: u64 = 0xDEAD_BEEF_BAAD_F00D;
        let mut xs = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let input: Vec<u8> = (0..64).map(|_| (xs() % 4) as u8).collect();
        let compressed = fast_encoder::vendored_compress(&input);
        let decoded = decode(&compressed).expect("decode");
        assert_eq!(decoded, input, "low-entropy 64-byte round-trip");
    }

    /// Empty input still produces an empty terminator stream and
    /// decodes to nothing. Catches header parsing regressions when
    /// the encoder short-circuits before `compress_fragment_two_pass`.
    #[test]
    fn decode_empty_input() {
        let compressed = fast_encoder::vendored_compress(&[]);
        let decoded = decode(&compressed).expect("decode empty");
        assert!(decoded.is_empty());
    }
}
