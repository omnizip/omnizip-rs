//! RAR3 LZSS unpackers — ports of unrar's unpack15.cpp, unpack20.cpp,
//! unpack30.cpp plus the shared helpers from unpack.cpp/unpackinline.cpp
//! (bit input with the 0x8000 sliding buffer and its exact refill
//! semantics, Huffman `DecodeTable`, `CopyString`, filter plumbing).
//! PPMd blocks dispatch to [`crate::rar3_ppmd`]; VM filters to
//! [`crate::rar3_vm`].
#![forbid(unsafe_code)]

use crate::rar3_ppmd::{ByteSource, ModelPPM};
use crate::rar3_vm::{read_data as vm_read_data, RarVm, VmFilter};
use omnizip_archive_core::ArchiveError;
use std::sync::OnceLock;

const MAX_SIZE: usize = 0x8000;
const MAX_QUICK_DECODE_BITS: u32 = 9;
const MAX3_INC_LZ_MATCH: usize = 0x104;
const MAX_INC_LZ_MATCH: usize = 0x1004;
const LOW_DIST_REP_COUNT: u32 = 16;
const MAX3_UNPACK_FILTERS: usize = 8192;

const NC30: usize = 299;
const DC30: usize = 60;
const LDC30: usize = 17;
const RC30: usize = 28;
const BC30: usize = 20;
const HUFF_TABLE_SIZE30: usize = NC30 + DC30 + RC30 + LDC30;

const NC20: usize = 298;
const DC20: usize = 48;
const RC20: usize = 28;
const BC20: usize = 19;
const MC20: usize = 257;

const LARGEST_TABLE_SIZE: usize = 306;

// ------------------------------------------------------------------
// Bit input with the reference's 0x8000 sliding buffer.

pub struct InBuf30 {
    pub buf: Vec<u8>,
    pub in_addr: usize,
    pub in_bit: u32,
    pub read_top: usize,
    pub read_border: isize,
    src: Vec<u8>,
    src_pos: usize,
    encrypted: bool,
}

impl InBuf30 {
    pub fn new(src: Vec<u8>, encrypted: bool) -> Self {
        Self {
            buf: vec![0u8; MAX_SIZE + 8],
            in_addr: 0,
            in_bit: 0,
            read_top: 0,
            read_border: 0,
            src,
            src_pos: 0,
            encrypted,
        }
    }

    /// Move forward by `bits` bits.
    pub fn addbits(&mut self, bits: u32) {
        let bits = bits + self.in_bit;
        self.in_addr += (bits >> 3) as usize;
        self.in_bit = bits & 7;
    }

    /// 16 bits from the current position (MSB first).
    pub fn getbits(&self) -> u32 {
        let a = self.in_addr;
        let mut field = u32::from(self.buf[a]) << 16;
        field |= u32::from(self.buf[a + 1]) << 8;
        field |= u32::from(self.buf[a + 2]);
        (field >> (8 - self.in_bit)) & 0xFFFF
    }

    /// `Unpack::GetChar`.
    pub fn get_char(&mut self) -> u8 {
        if self.in_addr > MAX_SIZE - 30 {
            self.unp_read_buf();
            if self.in_addr >= MAX_SIZE {
                return 0;
            }
        }
        let b = self.buf[self.in_addr];
        self.in_addr += 1;
        b
    }

    /// `Unpack::UnpReadBuf30`. Always returns true for the in-memory
    /// source (missing volumes are impossible at this layer).
    pub fn unp_read_buf(&mut self) -> bool {
        let data_size = self.read_top as isize - self.in_addr as isize;
        if data_size < 0 {
            return false;
        }
        let mut data_size = data_size as usize;
        if self.in_addr > MAX_SIZE / 2 {
            if data_size > 0 {
                self.buf
                    .copy_within(self.in_addr..self.in_addr + data_size, 0);
            }
            self.in_addr = 0;
            self.read_top = data_size;
        } else {
            data_size = self.read_top;
        }
        let space = MAX_SIZE - data_size;
        let mut count = space.min(self.src.len() - self.src_pos);
        if self.encrypted {
            count &= !15;
        }
        self.buf[data_size..data_size + count]
            .copy_from_slice(&self.src[self.src_pos..self.src_pos + count]);
        self.src_pos += count;
        self.read_top += count;
        self.read_border = self.read_top as isize - 30;
        true
    }
}

impl ByteSource for InBuf30 {
    fn get_char(&mut self) -> u8 {
        InBuf30::get_char(self)
    }
}

// ------------------------------------------------------------------
// Huffman decode table (unpack.cpp MakeDecodeTables).

#[derive(Clone)]
pub struct DecodeTable {
    max_num: usize,
    decode_len: [u32; 16],
    decode_pos: [u32; 16],
    quick_bits: u32,
    quick_len: [u8; 1 << MAX_QUICK_DECODE_BITS],
    quick_num: [u16; 1 << MAX_QUICK_DECODE_BITS],
    decode_num: [u16; LARGEST_TABLE_SIZE],
}

impl DecodeTable {
    fn zeroed() -> Self {
        Self {
            max_num: 0,
            decode_len: [0; 16],
            decode_pos: [0; 16],
            quick_bits: 0,
            quick_len: [0; 1 << MAX_QUICK_DECODE_BITS],
            quick_num: [0; 1 << MAX_QUICK_DECODE_BITS],
            decode_num: [0; LARGEST_TABLE_SIZE],
        }
    }
}

fn make_decode_tables(length_table: &[u8], dec: &mut DecodeTable, size: usize) {
    dec.max_num = size;
    let mut length_count = [0u32; 16];
    for i in 0..size {
        length_count[(length_table[i] & 0xF) as usize] += 1;
    }
    length_count[0] = 0;
    dec.decode_num[..size].fill(0);
    dec.decode_pos[0] = 0;
    dec.decode_len[0] = 0;
    let mut upper_limit = 0u32;
    for i in 1..16usize {
        upper_limit += length_count[i];
        let left_aligned = upper_limit << (16 - i);
        upper_limit *= 2;
        dec.decode_len[i] = left_aligned;
        dec.decode_pos[i] = dec.decode_pos[i - 1] + length_count[i - 1];
    }
    let mut copy_decode_pos = dec.decode_pos;
    for i in 0..size {
        let cur = (length_table[i] & 0xF) as usize;
        if cur != 0 {
            let last_pos = copy_decode_pos[cur] as usize;
            dec.decode_num[last_pos] = i as u16;
            copy_decode_pos[cur] += 1;
        }
    }
    dec.quick_bits = match size {
        NC30 | NC20 => MAX_QUICK_DECODE_BITS,
        _ => MAX_QUICK_DECODE_BITS.saturating_sub(3),
    };
    let quick_data_size = 1usize << dec.quick_bits;
    let mut cur_bit_length = 1u32;
    for code in 0..quick_data_size {
        let bit_field = (code as u32) << (16 - dec.quick_bits);
        while (cur_bit_length as usize) < dec.decode_len.len()
            && bit_field >= dec.decode_len[cur_bit_length as usize]
        {
            cur_bit_length += 1;
        }
        dec.quick_len[code] = cur_bit_length as u8;
        let mut dist = bit_field.wrapping_sub(dec.decode_len[(cur_bit_length - 1) as usize]);
        dist >>= 16 - cur_bit_length;
        dec.quick_num[code] = if (cur_bit_length as usize) < dec.decode_pos.len() {
            let pos = dec.decode_pos[cur_bit_length as usize].wrapping_add(dist);
            if (pos as usize) < size {
                dec.decode_num[pos as usize]
            } else {
                0
            }
        } else {
            0
        };
    }
}

fn decode_number(inp: &mut InBuf30, dec: &DecodeTable) -> u32 {
    let bit_field = inp.getbits() & 0xFFFE;
    if bit_field < dec.decode_len[dec.quick_bits as usize] {
        let code = (bit_field >> (16 - dec.quick_bits)) as usize;
        inp.addbits(u32::from(dec.quick_len[code]));
        return u32::from(dec.quick_num[code]);
    }
    let mut bits = 15u32;
    for i in (dec.quick_bits + 1)..15 {
        if bit_field < dec.decode_len[i as usize] {
            bits = i;
            break;
        }
    }
    inp.addbits(bits);
    let mut dist = bit_field.wrapping_sub(dec.decode_len[(bits - 1) as usize]);
    dist >>= 16 - bits;
    let pos = dec.decode_pos[bits as usize].wrapping_add(dist);
    let pos = if pos as usize >= dec.max_num { 0 } else { pos };
    u32::from(dec.decode_num[pos as usize])
}

#[derive(Clone)]
struct BlockTables30 {
    ld: DecodeTable,
    dd: DecodeTable,
    ldd: DecodeTable,
    rd: DecodeTable,
    bd: DecodeTable,
}

impl BlockTables30 {
    fn zeroed() -> Self {
        Self {
            ld: DecodeTable::zeroed(),
            dd: DecodeTable::zeroed(),
            ldd: DecodeTable::zeroed(),
            rd: DecodeTable::zeroed(),
            bd: DecodeTable::zeroed(),
        }
    }
}

fn dist_tables() -> &'static ([u32; DC30], [u32; DC30]) {
    static TABLES: OnceLock<([u32; DC30], [u32; DC30])> = OnceLock::new();
    TABLES.get_or_init(|| {
        static D_BIT_LENGTH_COUNTS: [u32; 19] =
            [4, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 14, 0, 12];
        let mut decode = [0u32; DC30];
        let mut bits = [0u32; DC30];
        let mut dist = 0u32;
        let mut bit_length = 0u32;
        let mut slot = 0usize;
        for &count in D_BIT_LENGTH_COUNTS.iter() {
            for _ in 0..count {
                decode[slot] = dist;
                bits[slot] = bit_length;
                slot += 1;
                dist += 1 << bit_length;
            }
            bit_length += 1;
        }
        (decode, bits)
    })
}

// ------------------------------------------------------------------
// Filters (UnpackFilter30).

#[derive(Clone)]
struct StackFilter {
    block_start: u32,
    block_length: u32,
    next_window: bool,
    prg_type: VmFilter,
    init_r: [u32; 7],
}

#[derive(Clone, Copy, Default, PartialEq)]
enum BlockType {
    #[default]
    Lz,
    Ppm,
}

// RAR 2.0 audio state.
#[derive(Clone, Default)]
struct AudioVariables {
    k1: i32,
    k2: i32,
    k3: i32,
    k4: i32,
    k5: i32,
    d1: i32,
    d2: i32,
    d3: i32,
    d4: i32,
    last_delta: i32,
    dif: [u32; 11],
    byte_count: u32,
    last_char: i32,
}

// v15 static decode tables (unpack15.cpp top).
static DEC_L1: [u32; 11] = [
    0x8000, 0xA000, 0xC000, 0xD000, 0xE000, 0xEA00, 0xEE00, 0xF000, 0xF200, 0xF200, 0xFFFF,
];
static POS_L1: [u32; 13] = [0, 0, 0, 2, 3, 5, 7, 11, 16, 20, 24, 32, 32];
static DEC_L2: [u32; 10] = [
    0xA000, 0xC000, 0xD000, 0xE000, 0xEA00, 0xEE00, 0xF000, 0xF200, 0xF240, 0xFFFF,
];
static POS_L2: [u32; 13] = [0, 0, 0, 0, 5, 7, 9, 13, 18, 22, 26, 34, 36];
static DEC_HF0: [u32; 9] = [
    0x8000, 0xC000, 0xE000, 0xF200, 0xF200, 0xF200, 0xF200, 0xF200, 0xFFFF,
];
static POS_HF0: [u32; 13] = [0, 0, 0, 0, 0, 8, 16, 24, 33, 33, 33, 33, 33];
static DEC_HF1: [u32; 8] = [
    0x2000, 0xC000, 0xE000, 0xF000, 0xF200, 0xF200, 0xF7E0, 0xFFFF,
];
static POS_HF1: [u32; 13] = [0, 0, 0, 0, 0, 0, 4, 44, 60, 76, 80, 80, 127];
static DEC_HF2: [u32; 8] = [
    0x1000, 0x2400, 0x8000, 0xC000, 0xFA00, 0xFFFF, 0xFFFF, 0xFFFF,
];
static POS_HF2: [u32; 13] = [0, 0, 0, 0, 0, 0, 2, 7, 53, 117, 233, 0, 0];
static DEC_HF3: [u32; 7] = [0x800, 0x2400, 0xEE00, 0xFE80, 0xFFFF, 0xFFFF, 0xFFFF];
static POS_HF3: [u32; 13] = [0, 0, 0, 0, 0, 0, 0, 2, 16, 218, 251, 0, 0];
static DEC_HF4: [u32; 6] = [0xFF00, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF];
static POS_HF4: [u32; 13] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0];

static SHORT_LEN1: [u32; 15] = [1, 3, 4, 4, 5, 6, 7, 8, 8, 4, 4, 5, 6, 6, 4];
static SHORT_LEN2: [u32; 15] = [2, 3, 3, 3, 4, 4, 5, 6, 6, 4, 4, 5, 6, 6, 4];

static L_DECODE: [u32; 28] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112,
    128, 160, 192, 224,
];
static L_BITS: [u32; 28] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5,
];
static SD_DECODE: [u32; 8] = [0, 4, 8, 16, 32, 64, 128, 192];
static SD_BITS: [u32; 8] = [2, 2, 3, 4, 5, 6, 6, 6];

static D20_DECODE: [u32; 48] = [
    0, 1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536,
    2048, 3072, 4096, 6144, 8192, 12288, 16384, 24576, 32768, 49152, 65536, 98304, 131072,
    196608, 262144, 327680, 393216, 458752, 524288, 589824, 655360, 720896, 786432, 851968,
    917504, 983040,
];
static D20_BITS: [u32; 48] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
    13, 13, 14, 14, 15, 15, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16,
];

// ------------------------------------------------------------------
// The unpacker.

pub struct Unpacker30 {
    window: Vec<u8>,
    max_win_size: usize,
    unp_ptr: usize,
    wr_ptr: usize,
    prev_ptr: usize,
    first_win_done: bool,
    old_dist: [usize; 4],
    old_dist_ptr: usize,
    last_dist: u32,
    last_length: u32,

    tables: BlockTables30,
    tables_read3: bool,
    unp_old_table: [u8; HUFF_TABLE_SIZE30],
    block_type: BlockType,
    ppm_esc_char: u8,
    ppm: ModelPPM,
    prev_low_dist: u32,
    low_dist_rep_count: u32,

    vm: RarVm,
    filters: Vec<VmFilter>,
    prg_stack: Vec<Option<StackFilter>>,
    old_filter_lengths: Vec<u32>,
    last_filter: usize,

    inp: InBuf30,
    out: Vec<u8>,
    written_file_size: i64,
    dest_unp_size: i64,

    // RAR 2.0 state.
    tables_read2: bool,
    unp_audio_block: bool,
    unp_channels: usize,
    unp_cur_channel: usize,
    unp_channel_delta: i32,
    md: [DecodeTable; 4],
    unp_old_table20: [u8; MC20 * 4],
    aud_v: [AudioVariables; 4],

    // RAR 1.5 state.
    ch_set: [u16; 256],
    ch_set_a: [u16; 256],
    ch_set_b: [u16; 256],
    ch_set_c: [u16; 256],
    nto_pl: [u8; 256],
    nto_pl_b: [u8; 256],
    nto_pl_c: [u8; 256],
    flag_buf: u32,
    avr_plc: u32,
    avr_plc_b: u32,
    avr_ln1: u32,
    avr_ln2: u32,
    avr_ln3: u32,
    buf60: i32,
    num_huf: i32,
    st_mode: i32,
    l_count: i32,
    flags_cnt: i32,
    nhfb: u32,
    nlzb: u32,
    max_dist3: u32,
}

impl Unpacker30 {
    #[must_use]
    pub fn new() -> Self {
        Self {
            window: Vec::new(),
            max_win_size: 0,
            unp_ptr: 0,
            wr_ptr: 0,
            prev_ptr: 0,
            first_win_done: false,
            old_dist: [usize::MAX; 4],
            old_dist_ptr: 0,
            last_dist: u32::MAX,
            last_length: 0,
            tables: BlockTables30::zeroed(),
            tables_read3: false,
            unp_old_table: [0; HUFF_TABLE_SIZE30],
            block_type: BlockType::Lz,
            ppm_esc_char: 2,
            ppm: ModelPPM::new(),
            prev_low_dist: 0,
            low_dist_rep_count: 0,
            vm: RarVm::new(),
            filters: Vec::new(),
            prg_stack: Vec::new(),
            old_filter_lengths: Vec::new(),
            last_filter: 0,
            inp: InBuf30::new(Vec::new(), false),
            out: Vec::new(),
            written_file_size: 0,
            dest_unp_size: 0,
            tables_read2: false,
            unp_audio_block: false,
            unp_channels: 1,
            unp_cur_channel: 0,
            unp_channel_delta: 0,
            md: [
                DecodeTable::zeroed(),
                DecodeTable::zeroed(),
                DecodeTable::zeroed(),
                DecodeTable::zeroed(),
            ],
            unp_old_table20: [0; MC20 * 4],
            aud_v: [
                AudioVariables::default(),
                AudioVariables::default(),
                AudioVariables::default(),
                AudioVariables::default(),
            ],
            ch_set: [0; 256],
            ch_set_a: [0; 256],
            ch_set_b: [0; 256],
            ch_set_c: [0; 256],
            nto_pl: [0; 256],
            nto_pl_b: [0; 256],
            nto_pl_c: [0; 256],
            flag_buf: 0,
            avr_plc: 0,
            avr_plc_b: 0,
            avr_ln1: 0,
            avr_ln2: 0,
            avr_ln3: 0,
            buf60: 0,
            num_huf: 0,
            st_mode: 0,
            l_count: 0,
            flags_cnt: 0,
            nhfb: 0,
            nlzb: 0,
            max_dist3: 0,
        }
    }

    fn mask(&self) -> usize {
        self.max_win_size - 1
    }

    /// Unpack one entry. `solid` continues the previous window/tables/
    /// PPM state (the caller decodes solid runs in archive order).
    ///
    /// # Errors
    ///
    /// [`ArchiveError`] on unsupported versions.
    pub fn do_unpack(
        &mut self,
        unp_ver: u8,
        solid: bool,
        packed: Vec<u8>,
        encrypted: bool,
        dest_unp_size: i64,
        win_size: usize,
    ) -> Result<Vec<u8>, ArchiveError> {
        let win = win_size.max(0x40000).min(0x4000_0000);
        if !solid || self.window.is_empty() {
            self.max_win_size = win;
            self.window = vec![0u8; win];
        }
        self.out = Vec::new();
        self.inp = InBuf30::new(packed, encrypted);
        self.dest_unp_size = dest_unp_size;

        self.unp_init_data(solid);
        match unp_ver {
            15 => self.unpack15(solid),
            20 | 26 => self.unpack20(solid),
            29 => self.unpack29(solid),
            other => {
                return Err(ArchiveError::UnsupportedFeature {
                    reason: format!("rar4: unpack version {other}"),
                });
            }
        }
        Ok(std::mem::take(&mut self.out))
    }

    fn unp_init_data(&mut self, solid: bool) {
        if !solid {
            self.old_dist = [usize::MAX; 4];
            self.old_dist_ptr = 0;
            self.last_dist = u32::MAX;
            self.last_length = 0;
            self.tables = BlockTables30::zeroed();
            self.unp_ptr = 0;
            self.wr_ptr = 0;
            self.prev_ptr = 0;
            self.first_win_done = false;
        }
        self.init_filters30(solid);
        self.inp.in_addr = 0;
        self.inp.in_bit = 0;
        self.written_file_size = 0;
        self.inp.read_top = 0;
        self.inp.read_border = 0;
        self.unp_init_data20(solid);
        self.unp_init_data30(solid);
        self.unp_init_data15(solid);
    }

    fn unp_init_data30(&mut self, solid: bool) {
        if !solid {
            self.tables_read3 = false;
            self.unp_old_table = [0; HUFF_TABLE_SIZE30];
            self.ppm_esc_char = 2;
            self.block_type = BlockType::Lz;
        }
    }

    fn init_filters30(&mut self, solid: bool) {
        if !solid {
            self.old_filter_lengths.clear();
            self.last_filter = 0;
            self.filters.clear();
        }
        self.prg_stack.clear();
    }

    fn unp_init_data20(&mut self, solid: bool) {
        if !solid {
            self.tables_read2 = false;
            self.unp_audio_block = false;
            self.unp_channel_delta = 0;
            self.unp_cur_channel = 0;
            self.unp_channels = 1;
            self.aud_v = [
                AudioVariables::default(),
                AudioVariables::default(),
                AudioVariables::default(),
                AudioVariables::default(),
            ];
            self.unp_old_table20 = [0; MC20 * 4];
            for t in &mut self.md {
                *t = DecodeTable::zeroed();
            }
        }
    }

    fn unp_init_data15(&mut self, solid: bool) {
        if !solid {
            self.avr_plc_b = 0;
            self.avr_ln1 = 0;
            self.avr_ln2 = 0;
            self.avr_ln3 = 0;
            self.num_huf = 0;
            self.buf60 = 0;
            self.avr_plc = 0x3500;
            self.max_dist3 = 0x2001;
            self.nhfb = 0x80;
            self.nlzb = 0x80;
        }
        self.flags_cnt = 0;
        self.flag_buf = 0;
        self.st_mode = 0;
        self.l_count = 0;
        self.inp.read_top = 0;
    }

    // -- output ---------------------------------------------------

    fn unp_write_area(&mut self, start_ptr: usize, end_ptr: usize) {
        if end_ptr < start_ptr {
            let head: Vec<u8> = self.window[start_ptr..].to_vec();
            self.out.extend_from_slice(&head);
            let tail: Vec<u8> = self.window[..end_ptr].to_vec();
            self.out.extend_from_slice(&tail);
        } else {
            let chunk: Vec<u8> = self.window[start_ptr..end_ptr].to_vec();
            self.out.extend_from_slice(&chunk);
        }
    }

    fn unp_write_buf20(&mut self) {
        if self.unp_ptr < self.wr_ptr {
            self.unp_write_area(self.wr_ptr, self.max_win_size);
            self.unp_write_area(0, self.unp_ptr);
        } else {
            self.unp_write_area(self.wr_ptr, self.unp_ptr);
        }
        self.wr_ptr = self.unp_ptr;
    }

    fn unp_write_buf30(&mut self) {
        let mask = self.mask() as u32;
        let mut written_border = self.wr_ptr as u32;
        let mut write_size = (self.unp_ptr as u32).wrapping_sub(written_border) & mask;
        let mut i = 0usize;
        while i < self.prg_stack.len() {
            let Some(flt) = self.prg_stack[i].clone() else {
                i += 1;
                continue;
            };
            if flt.next_window {
                if let Some(f) = &mut self.prg_stack[i] {
                    f.next_window = false;
                }
                i += 1;
                continue;
            }
            let block_start = flt.block_start;
            let block_length = flt.block_length;
            if block_start.wrapping_sub(written_border) & mask < write_size {
                if written_border != block_start {
                    self.unp_write_area(written_border as usize, block_start as usize);
                    written_border = block_start;
                    write_size = (self.unp_ptr as u32).wrapping_sub(written_border) & mask;
                }
                if block_length <= write_size {
                    let block_end = block_start.wrapping_add(block_length) & mask;
                    let (first, second) = if block_start < block_end || block_end == 0 {
                        (
                            self.window
                                [block_start as usize..block_start as usize + block_length as usize]
                                .to_vec(),
                            Vec::new(),
                        )
                    } else {
                        (
                            self.window[block_start as usize..].to_vec(),
                            self.window[..block_end as usize].to_vec(),
                        )
                    };
                    self.vm.set_memory(0, &first);
                    if !second.is_empty() {
                        self.vm.set_memory(first.len(), &second);
                    }

                    let mut init_r = flt.init_r;
                    init_r[6] = self.written_file_size as u32;
                    let mut vout = self.vm.execute(flt.prg_type, &init_r);
                    self.prg_stack[i] = None;

                    let mut filtered_offset = vout.offset;
                    let mut filtered_size = vout.size;
                    while i + 1 < self.prg_stack.len() {
                        let chain = match &self.prg_stack[i + 1] {
                            Some(nf) => {
                                nf.block_start == block_start
                                    && u32::try_from(filtered_size) == Ok(nf.block_length)
                                    && !nf.next_window
                            }
                            None => false,
                        };
                        if !chain {
                            break;
                        }
                        let nf = self.prg_stack[i + 1].clone().expect("checked");
                        let chunk = self.vm.mem[filtered_offset..filtered_offset + filtered_size]
                            .to_vec();
                        self.vm.set_memory(0, &chunk);
                        let mut init_r = nf.init_r;
                        init_r[6] = self.written_file_size as u32;
                        vout = self.vm.execute(nf.prg_type, &init_r);
                        filtered_offset = vout.offset;
                        filtered_size = vout.size;
                        self.prg_stack[i + 1] = None;
                        i += 1;
                    }
                    let data: Vec<u8> =
                        self.vm.mem[filtered_offset..filtered_offset + filtered_size].to_vec();
                    self.out.extend_from_slice(&data);
                    self.written_file_size += filtered_size as i64;
                    written_border = block_end;
                    write_size = (self.unp_ptr as u32).wrapping_sub(written_border) & mask;
                } else {
                    // Filter crosses the write border; process next time.
                    for f in self.prg_stack[i..].iter_mut().flatten() {
                        f.next_window = false;
                    }
                    self.wr_ptr = written_border as usize;
                    return;
                }
            }
            i += 1;
        }
        self.unp_write_area(written_border as usize, self.unp_ptr);
        self.wr_ptr = self.unp_ptr;
    }

    // -- shared LZ helpers ----------------------------------------

    fn insert_old_dist(&mut self, distance: usize) {
        self.old_dist[3] = self.old_dist[2];
        self.old_dist[2] = self.old_dist[1];
        self.old_dist[1] = self.old_dist[0];
        self.old_dist[0] = distance;
    }

    fn wrap_up(&self, pos: usize) -> usize {
        if pos >= self.max_win_size {
            pos - self.max_win_size
        } else {
            pos
        }
    }

    /// `Unpack::CopyString` (v29/v20 path with wrap safety).
    fn copy_string(&mut self, mut length: u32, distance: usize) {
        let mut src_ptr = self.unp_ptr.wrapping_sub(distance);
        if distance > self.unp_ptr {
            src_ptr = src_ptr.wrapping_add(self.max_win_size);
            if distance > self.max_win_size || !self.first_win_done {
                for _ in 0..length {
                    self.window[self.unp_ptr] = 0;
                    self.unp_ptr = self.wrap_up(self.unp_ptr + 1);
                }
                return;
            }
        }
        if src_ptr < self.max_win_size - MAX_INC_LZ_MATCH
            && self.unp_ptr < self.max_win_size - MAX_INC_LZ_MATCH
        {
            let src = src_ptr;
            let dest = self.unp_ptr;
            self.unp_ptr += length as usize;
            if distance < length as usize {
                // Overlapping (RLE-like): byte-by-byte forward so copied
                // bytes propagate, like the reference.
                for n in 0..length as usize {
                    self.window[dest + n] = self.window[src + n];
                }
            } else {
                self.window.copy_within(src..src + length as usize, dest);
            }
        } else {
            while length > 0 {
                length -= 1;
                let b = self.window[self.wrap_up(src_ptr)];
                src_ptr += 1;
                self.window[self.unp_ptr] = b;
                self.unp_ptr = self.wrap_up(self.unp_ptr + 1);
            }
        }
    }

    // -- v29 ------------------------------------------------------

    fn unpack29(&mut self, solid: bool) {
        let (d_decode, d_bits) = dist_tables();

        self.inp.unp_read_buf();
        if (!solid || !self.tables_read3) && !self.read_tables30() {
            return;
        }

        loop {
            self.unp_ptr &= self.mask();
            self.first_win_done |= self.prev_ptr > self.unp_ptr;
            self.prev_ptr = self.unp_ptr;

            if self.inp.in_addr as isize > self.inp.read_border {
                if !self.inp.unp_read_buf() {
                    break;
                }
            }
            if self.wr_ptr.wrapping_sub(self.unp_ptr) & self.mask() <= MAX3_INC_LZ_MATCH
                && self.wr_ptr != self.unp_ptr
            {
                self.unp_write_buf30();
                if self.written_file_size > self.dest_unp_size {
                    return;
                }
            }
            if self.block_type == BlockType::Ppm {
                let ch = self.ppm.decode_char(&mut self.inp);
                if ch == -1 {
                    self.ppm.clean_up();
                    self.block_type = BlockType::Lz;
                    break;
                }
                if ch == i32::from(self.ppm_esc_char) {
                    let next_ch = self.safe_ppm_decode_char();
                    if next_ch == 0 {
                        if !self.read_tables30() {
                            break;
                        }
                        continue;
                    }
                    if next_ch == -1 {
                        break;
                    }
                    if next_ch == 2 {
                        break;
                    }
                    if next_ch == 3 {
                        if !self.read_vm_code_ppm() {
                            break;
                        }
                        continue;
                    }
                    if next_ch == 4 {
                        let mut distance = 0u32;
                        let mut length = 0u32;
                        let mut failed = false;
                        for i in 0..4 {
                            let ch = self.safe_ppm_decode_char();
                            if ch == -1 {
                                failed = true;
                                break;
                            }
                            if i == 3 {
                                length = ch as u8 as u32;
                            } else {
                                distance = (distance << 8) + ch as u8 as u32;
                            }
                        }
                        if failed {
                            break;
                        }
                        self.copy_string(length + 32, distance as usize + 2);
                        continue;
                    }
                    if next_ch == 5 {
                        let length = self.safe_ppm_decode_char();
                        if length == -1 {
                            break;
                        }
                        self.copy_string(length as u32 + 4, 1);
                        continue;
                    }
                }
                self.window[self.unp_ptr] = ch as u8;
                self.unp_ptr += 1;
                continue;
            }

            let number = decode_number(&mut self.inp, &self.tables.ld);
            if number < 256 {
                self.window[self.unp_ptr] = number as u8;
                self.unp_ptr += 1;
                continue;
            }
            if number >= 271 {
                let n = (number - 271) as usize;
                let mut length = L_DECODE[n] + 3;
                let bits = L_BITS[n];
                if bits > 0 {
                    length += self.inp.getbits() >> (16 - bits);
                    self.inp.addbits(bits);
                }

                let dist_number = decode_number(&mut self.inp, &self.tables.dd) as usize;
                let mut distance = d_decode[dist_number] + 1;
                let bits = d_bits[dist_number];
                if bits > 0 {
                    if dist_number > 9 {
                        if bits > 4 {
                            distance += (self.inp.getbits() >> (20 - bits)) << 4;
                            self.inp.addbits(bits - 4);
                        }
                        if self.low_dist_rep_count > 0 {
                            self.low_dist_rep_count -= 1;
                            distance += self.prev_low_dist;
                        } else {
                            let low_dist = decode_number(&mut self.inp, &self.tables.ldd);
                            if low_dist == 16 {
                                self.low_dist_rep_count = LOW_DIST_REP_COUNT - 1;
                                distance += self.prev_low_dist;
                            } else {
                                distance += low_dist;
                                self.prev_low_dist = low_dist;
                            }
                        }
                    } else {
                        distance += self.inp.getbits() >> (16 - bits);
                        self.inp.addbits(bits);
                    }
                }

                if distance >= 0x2000 {
                    length += 1;
                    if distance >= 0x40000 {
                        length += 1;
                    }
                }

                self.insert_old_dist(distance as usize);
                self.last_length = length;
                self.copy_string(length, distance as usize);
                continue;
            }
            if number == 256 {
                if !self.read_end_of_block() {
                    break;
                }
                continue;
            }
            if number == 257 {
                if !self.read_vm_code() {
                    break;
                }
                continue;
            }
            if number == 258 {
                if self.last_length != 0 {
                    let d = self.old_dist[0];
                    let l = self.last_length;
                    self.copy_string(l, d);
                }
                continue;
            }
            if number < 263 {
                let dist_num = (number - 259) as usize;
                let distance = self.old_dist[dist_num];
                for i in (1..=dist_num).rev() {
                    self.old_dist[i] = self.old_dist[i - 1];
                }
                self.old_dist[0] = distance;

                let length_number = decode_number(&mut self.inp, &self.tables.rd) as usize;
                let mut length = L_DECODE[length_number] + 2;
                let bits = L_BITS[length_number];
                if bits > 0 {
                    length += self.inp.getbits() >> (16 - bits);
                    self.inp.addbits(bits);
                }
                self.last_length = length;
                self.copy_string(length, distance);
                continue;
            }
            if number < 272 {
                let n = (number - 263) as usize;
                let mut distance = SD_DECODE[n] + 1;
                let bits = SD_BITS[n];
                if bits > 0 {
                    distance += self.inp.getbits() >> (16 - bits);
                    self.inp.addbits(bits);
                }
                self.insert_old_dist(distance as usize);
                self.last_length = 2;
                self.copy_string(2, distance as usize);
                continue;
            }
        }
        self.unp_write_buf30();
    }

    fn safe_ppm_decode_char(&mut self) -> i32 {
        let ch = self.ppm.decode_char(&mut self.inp);
        if ch == -1 {
            self.ppm.clean_up();
            self.block_type = BlockType::Lz;
        }
        ch
    }

    fn read_end_of_block(&mut self) -> bool {
        let bit_field = self.inp.getbits();
        let new_table;
        let new_file;
        if bit_field & 0x8000 != 0 {
            new_table = true;
            new_file = false;
            self.inp.addbits(1);
        } else {
            new_file = true;
            new_table = bit_field & 0x4000 != 0;
            self.inp.addbits(2);
        }
        self.tables_read3 = !new_table;
        if new_file {
            return false;
        }
        self.read_tables30()
    }

    fn read_tables30(&mut self) -> bool {
        let mut bit_length = [0u8; BC30];
        let mut table = [0u8; HUFF_TABLE_SIZE30];
        if self.inp.in_addr as isize > self.inp.read_top as isize - 25 {
            if !self.inp.unp_read_buf() {
                return false;
            }
        }
        self.inp.addbits((8 - self.inp.in_bit) & 7);
        let bit_field = self.inp.getbits();
        if bit_field & 0x8000 != 0 {
            self.block_type = BlockType::Ppm;
            let ok = self.ppm.decode_init(&mut self.inp, &mut self.ppm_esc_char);
            if std::env::var("OZIP_DBG").is_ok() {
                eprintln!("TABLES30->PPM decode_init={ok}");
            }
            return ok;
        }
        self.block_type = BlockType::Lz;
        self.prev_low_dist = 0;
        self.low_dist_rep_count = 0;
        if bit_field & 0x4000 == 0 {
            self.unp_old_table = [0; HUFF_TABLE_SIZE30];
        }
        self.inp.addbits(2);

        let mut i = 0usize;
        while i < BC30 {
            let length = (self.inp.getbits() >> 12) as u8;
            self.inp.addbits(4);
            if length == 15 {
                let zero_count = (self.inp.getbits() >> 12) as u8;
                self.inp.addbits(4);
                if zero_count == 0 {
                    bit_length[i] = 15;
                    i += 1;
                } else {
                    for _ in 0..zero_count + 2 {
                        if i < bit_length.len() {
                            bit_length[i] = 0;
                            i += 1;
                        }
                    }
                }
            } else {
                bit_length[i] = length;
                i += 1;
            }
        }
        make_decode_tables(&bit_length, &mut self.tables.bd, BC30);

        let mut i = 0usize;
        while i < HUFF_TABLE_SIZE30 {
            if self.inp.in_addr as isize > self.inp.read_top as isize - 5 {
                if !self.inp.unp_read_buf() {
                    return false;
                }
            }
            let number = decode_number(&mut self.inp, &self.tables.bd);
            if number < 16 {
                table[i] = number.wrapping_add(u32::from(self.unp_old_table[i])) as u8 & 0xF;
                i += 1;
            } else if number < 18 {
                let n = if number == 16 {
                    let n = (self.inp.getbits() >> 13) + 3;
                    self.inp.addbits(3);
                    n
                } else {
                    let n = (self.inp.getbits() >> 9) + 11;
                    self.inp.addbits(7);
                    n
                };
                if i == 0 {
                    return false;
                }
                for _ in 0..n {
                    if i >= HUFF_TABLE_SIZE30 {
                        break;
                    }
                    table[i] = table[i - 1];
                    i += 1;
                }
            } else {
                let n = if number == 18 {
                    let n = (self.inp.getbits() >> 13) + 3;
                    self.inp.addbits(3);
                    n
                } else {
                    let n = (self.inp.getbits() >> 9) + 11;
                    self.inp.addbits(7);
                    n
                };
                for _ in 0..n {
                    if i >= HUFF_TABLE_SIZE30 {
                        break;
                    }
                    table[i] = 0;
                    i += 1;
                }
            }
        }
        self.tables_read3 = true;
        if self.inp.in_addr > self.inp.read_top {
            return false;
        }
        make_decode_tables(&table[..NC30], &mut self.tables.ld, NC30);
        make_decode_tables(&table[NC30..NC30 + DC30], &mut self.tables.dd, DC30);
        make_decode_tables(
            &table[NC30 + DC30..NC30 + DC30 + LDC30],
            &mut self.tables.ldd,
            LDC30,
        );
        make_decode_tables(
            &table[NC30 + DC30 + LDC30..],
            &mut self.tables.rd,
            RC30,
        );
        self.unp_old_table = table;
        true
    }

    // -- VM code (filters) ----------------------------------------

    fn read_vm_code(&mut self) -> bool {
        let first_byte = self.inp.getbits() >> 8;
        self.inp.addbits(8);
        let mut length = (first_byte & 7) + 1;
        if length == 7 {
            length = (self.inp.getbits() >> 8) + 7;
            self.inp.addbits(8);
        } else if length == 8 {
            length = self.inp.getbits();
            self.inp.addbits(16);
        }
        if length == 0 {
            return false;
        }
        let mut code = vec![0u8; length as usize];
        for b in &mut code {
            if self.inp.in_addr as isize >= self.inp.read_top as isize - 1 {
                self.inp.unp_read_buf();
            }
            *b = (self.inp.getbits() >> 8) as u8;
            self.inp.addbits(8);
        }
        self.add_vm_code(first_byte, &code)
    }

    fn read_vm_code_ppm(&mut self) -> bool {
        let first_byte = self.safe_ppm_decode_char();
        if first_byte == -1 {
            return false;
        }
        let first_byte = first_byte as u8 as u32;
        let mut length = (first_byte & 7) + 1;
        if length == 7 {
            let b1 = self.safe_ppm_decode_char();
            if b1 == -1 {
                return false;
            }
            length = b1 as u32 + 7;
        } else if length == 8 {
            let b1 = self.safe_ppm_decode_char();
            if b1 == -1 {
                return false;
            }
            let b2 = self.safe_ppm_decode_char();
            if b2 == -1 {
                return false;
            }
            length = b1 as u32 * 256 + b2 as u32;
        }
        if length == 0 {
            return false;
        }
        let mut code = vec![0u8; length as usize];
        for b in &mut code {
            let ch = self.safe_ppm_decode_char();
            if ch == -1 {
                return false;
            }
            *b = ch as u8;
        }
        self.add_vm_code(first_byte, &code)
    }

    fn add_vm_code(&mut self, first_byte: u32, code: &[u8]) -> bool {
        let mut vinp = InBuf30::new(Vec::new(), false);
        let copy_len = code.len().min(MAX_SIZE);
        vinp.buf[..copy_len].copy_from_slice(&code[..copy_len]);

        let mut filt_pos;
        if first_byte & 0x80 != 0 {
            filt_pos = vm_read_data(&mut vinp);
            if filt_pos == 0 {
                self.init_filters30(false);
            } else {
                filt_pos -= 1;
            }
        } else {
            filt_pos = self.last_filter as u32;
        }
        if filt_pos as usize > self.filters.len()
            || filt_pos as usize > self.old_filter_lengths.len()
        {
            return false;
        }
        let filt_pos = filt_pos as usize;
        self.last_filter = filt_pos;
        let new_filter = filt_pos == self.filters.len();

        let mut filter_type = VmFilter::None;
        if new_filter {
            if filt_pos > MAX3_UNPACK_FILTERS {
                return false;
            }
            self.old_filter_lengths.push(0);
        } else {
            filter_type = self.filters[filt_pos];
        }

        // Compact the stack (drop None entries).
        let mut compacted: Vec<Option<StackFilter>> = Vec::with_capacity(self.prg_stack.len() + 1);
        for item in self.prg_stack.drain(..) {
            if item.is_some() {
                compacted.push(item);
            }
        }
        self.prg_stack = compacted;

        let mut stack_filter = StackFilter {
            block_start: 0,
            block_length: 0,
            next_window: false,
            prg_type: VmFilter::None,
            init_r: [0; 7],
        };

        let mut block_start = vm_read_data(&mut vinp);
        if first_byte & 0x40 != 0 {
            block_start += 258;
        }
        stack_filter.block_start =
            block_start.wrapping_add(self.unp_ptr as u32) & self.mask() as u32;
        if first_byte & 0x20 != 0 {
            stack_filter.block_length = vm_read_data(&mut vinp);
            if filt_pos < self.old_filter_lengths.len() {
                self.old_filter_lengths[filt_pos] = stack_filter.block_length;
            }
        } else {
            stack_filter.block_length = if filt_pos < self.old_filter_lengths.len() {
                self.old_filter_lengths[filt_pos]
            } else {
                0
            };
        }

        stack_filter.next_window = self.wr_ptr != self.unp_ptr
            && (self.wr_ptr.wrapping_sub(self.unp_ptr) & self.mask()) as u32 <= block_start;

        stack_filter.init_r[4] = stack_filter.block_length;

        if first_byte & 0x10 != 0 {
            let init_mask = vinp.getbits() >> 9;
            vinp.addbits(7);
            for (i, r) in stack_filter.init_r.iter_mut().enumerate().take(7) {
                if init_mask & (1 << i) != 0 {
                    *r = vm_read_data(&mut vinp);
                }
            }
        }

        if new_filter {
            let vm_code_size = vm_read_data(&mut vinp) as usize;
            if vm_code_size >= 0x10000
                || vm_code_size == 0
                || vinp.in_addr + vm_code_size > code.len()
            {
                return false;
            }
            let mut vm_code = vec![0u8; vm_code_size];
            for b in &mut vm_code {
                if vinp.in_addr + 3 >= MAX_SIZE {
                    return false;
                }
                *b = (vinp.getbits() >> 8) as u8;
                vinp.addbits(8);
            }
            filter_type = crate::rar3_vm::prepare(&vm_code);
            self.filters.push(filter_type);
        }
        stack_filter.prg_type = filter_type;
        self.prg_stack.push(Some(stack_filter));
        true
    }

    // -- v20 ------------------------------------------------------

    fn copy_string20(&mut self, length: u32, distance: u32) {
        self.last_dist = distance;
        self.old_dist[self.old_dist_ptr] = distance as usize;
        self.old_dist_ptr += 1;
        self.old_dist_ptr &= 3;
        self.last_length = length;
        self.dest_unp_size -= length as i64;
        self.copy_string(length, distance as usize);
    }

    fn unpack20(&mut self, solid: bool) {
        self.inp.unp_read_buf();
        if (!solid || !self.tables_read2) && !self.read_tables20() {
            return;
        }
        self.dest_unp_size -= 1;

        while self.dest_unp_size >= 0 {
            self.unp_ptr &= self.mask();
            self.first_win_done |= self.prev_ptr > self.unp_ptr;
            self.prev_ptr = self.unp_ptr;

            if self.inp.in_addr as isize > self.inp.read_top as isize - 30 {
                if !self.inp.unp_read_buf() {
                    break;
                }
            }
            if self.wr_ptr.wrapping_sub(self.unp_ptr) & self.mask() < 270
                && self.wr_ptr != self.unp_ptr
            {
                self.unp_write_buf20();
            }
            if self.unp_audio_block {
                let audio_number =
                    decode_number(&mut self.inp, &self.md[self.unp_cur_channel]);
                if audio_number == 256 {
                    if !self.read_tables20() {
                        break;
                    }
                    continue;
                }
                let ch = self.decode_audio(audio_number as i32);
                self.window[self.unp_ptr] = ch;
                self.unp_ptr += 1;
                self.unp_cur_channel += 1;
                if self.unp_cur_channel == self.unp_channels {
                    self.unp_cur_channel = 0;
                }
                self.dest_unp_size -= 1;
                continue;
            }

            let number = decode_number(&mut self.inp, &self.tables.ld);
            if number < 256 {
                self.window[self.unp_ptr] = number as u8;
                self.unp_ptr += 1;
                self.dest_unp_size -= 1;
                continue;
            }
            if number > 269 {
                let n = (number - 270) as usize;
                let mut length = L_DECODE[n] + 3;
                let bits = L_BITS[n];
                if bits > 0 {
                    length += self.inp.getbits() >> (16 - bits);
                    self.inp.addbits(bits);
                }
                let dist_number = decode_number(&mut self.inp, &self.tables.dd) as usize;
                let mut distance = D20_DECODE[dist_number] + 1;
                let bits = D20_BITS[dist_number];
                if bits > 0 {
                    distance += self.inp.getbits() >> (16 - bits);
                    self.inp.addbits(bits);
                }
                if distance >= 0x2000 {
                    length += 1;
                    if distance >= 0x40000 {
                        length += 1;
                    }
                }
                self.copy_string20(length, distance);
                continue;
            }
            if number == 269 {
                if !self.read_tables20() {
                    break;
                }
                continue;
            }
            if number == 256 {
                let l = self.last_length;
                let d = self.last_dist;
                self.copy_string20(l, d);
                continue;
            }
            if number < 261 {
                let idx = (self.old_dist_ptr + 4).wrapping_sub(number as usize - 256) & 3;
                let distance = self.old_dist[idx];
                let length_number = decode_number(&mut self.inp, &self.tables.rd) as usize;
                let mut length = L_DECODE[length_number] + 2;
                let bits = L_BITS[length_number];
                if bits > 0 {
                    length += self.inp.getbits() >> (16 - bits);
                    self.inp.addbits(bits);
                }
                if distance as u32 >= 0x101 {
                    length += 1;
                    if distance as u32 >= 0x2000 {
                        length += 1;
                        if distance as u32 >= 0x40000 {
                            length += 1;
                        }
                    }
                }
                self.copy_string20(length, distance as u32);
                continue;
            }
            if number < 270 {
                let n = (number - 261) as usize;
                let mut distance = SD_DECODE[n] + 1;
                let bits = SD_BITS[n];
                if bits > 0 {
                    distance += self.inp.getbits() >> (16 - bits);
                    self.inp.addbits(bits);
                }
                self.copy_string20(2, distance);
            }
        }
        self.read_last_tables();
        self.unp_write_buf20();
    }

    fn read_tables20(&mut self) -> bool {
        let mut bit_length = [0u8; BC20];
        let mut table = [0u8; MC20 * 4];
        if self.inp.in_addr as isize > self.inp.read_top as isize - 25 {
            if !self.inp.unp_read_buf() {
                return false;
            }
        }
        let bit_field = self.inp.getbits();
        self.unp_audio_block = bit_field & 0x8000 != 0;
        if bit_field & 0x4000 == 0 {
            self.unp_old_table20 = [0; MC20 * 4];
        }
        self.inp.addbits(2);

        let table_size;
        if self.unp_audio_block {
            self.unp_channels = (((bit_field >> 12) & 3) + 1) as usize;
            if self.unp_cur_channel >= self.unp_channels {
                self.unp_cur_channel = 0;
            }
            self.inp.addbits(2);
            table_size = MC20 * self.unp_channels;
        } else {
            table_size = NC20 + DC20 + RC20;
        }

        for b in bit_length.iter_mut().take(BC20) {
            *b = (self.inp.getbits() >> 12) as u8;
            self.inp.addbits(4);
        }
        make_decode_tables(&bit_length, &mut self.tables.bd, BC20);
        let mut i = 0usize;
        while i < table_size {
            if self.inp.in_addr as isize > self.inp.read_top as isize - 5 {
                if !self.inp.unp_read_buf() {
                    return false;
                }
            }
            let number = decode_number(&mut self.inp, &self.tables.bd);
            if number < 16 {
                table[i] = number.wrapping_add(u32::from(self.unp_old_table20[i])) as u8 & 0xF;
                i += 1;
            } else if number == 16 {
                let n = (self.inp.getbits() >> 14) + 3;
                self.inp.addbits(2);
                if i == 0 {
                    return false;
                }
                for _ in 0..n {
                    if i >= table_size {
                        break;
                    }
                    table[i] = table[i - 1];
                    i += 1;
                }
            } else {
                let n = if number == 17 {
                    let n = (self.inp.getbits() >> 13) + 3;
                    self.inp.addbits(3);
                    n
                } else {
                    let n = (self.inp.getbits() >> 9) + 11;
                    self.inp.addbits(7);
                    n
                };
                for _ in 0..n {
                    if i >= table_size {
                        break;
                    }
                    table[i] = 0;
                    i += 1;
                }
            }
        }
        self.tables_read2 = true;
        if self.inp.in_addr > self.inp.read_top {
            return true;
        }
        if self.unp_audio_block {
            for c in 0..self.unp_channels {
                make_decode_tables(
                    &table[c * MC20..(c + 1) * MC20],
                    &mut self.md[c],
                    MC20,
                );
            }
        } else {
            make_decode_tables(&table[..NC20], &mut self.tables.ld, NC20);
            make_decode_tables(&table[NC20..NC20 + DC20], &mut self.tables.dd, DC20);
            make_decode_tables(
                &table[NC20 + DC20..NC20 + DC20 + RC20],
                &mut self.tables.rd,
                RC20,
            );
        }
        self.unp_old_table20[..table_size].copy_from_slice(&table[..table_size]);
        true
    }

    fn read_last_tables(&mut self) {
        if self.inp.read_top as isize >= self.inp.in_addr as isize + 5 {
            if self.unp_audio_block {
                if decode_number(&mut self.inp, &self.md[self.unp_cur_channel]) == 256 {
                    let _ = self.read_tables20();
                }
            } else if decode_number(&mut self.inp, &self.tables.ld) == 269 {
                let _ = self.read_tables20();
            }
        }
    }

    fn decode_audio(&mut self, delta: i32) -> u8 {
        let cur = self.unp_cur_channel;
        let channel_delta = self.unp_channel_delta;
        let v = &mut self.aud_v[cur];
        v.byte_count += 1;
        v.d4 = v.d3;
        v.d3 = v.d2;
        v.d2 = v.last_delta - v.d1;
        v.d1 = v.last_delta;
        let mut p_ch: i32 = 8 * v.last_char
            + v.k1 * v.d1
            + v.k2 * v.d2
            + v.k3 * v.d3
            + v.k4 * v.d4
            + v.k5 * channel_delta;
        p_ch = (p_ch >> 3) & 0xFF;

        let ch = p_ch.wrapping_sub(delta) as u32;

        let d = (delta as i8 as i32) << 3;

        v.dif[0] = v.dif[0].wrapping_add(d.unsigned_abs());
        v.dif[1] = v.dif[1].wrapping_add((d - v.d1).unsigned_abs());
        v.dif[2] = v.dif[2].wrapping_add((d + v.d1).unsigned_abs());
        v.dif[3] = v.dif[3].wrapping_add((d - v.d2).unsigned_abs());
        v.dif[4] = v.dif[4].wrapping_add((d + v.d2).unsigned_abs());
        v.dif[5] = v.dif[5].wrapping_add((d - v.d3).unsigned_abs());
        v.dif[6] = v.dif[6].wrapping_add((d + v.d3).unsigned_abs());
        v.dif[7] = v.dif[7].wrapping_add((d - v.d4).unsigned_abs());
        v.dif[8] = v.dif[8].wrapping_add((d + v.d4).unsigned_abs());
        v.dif[9] = v.dif[9].wrapping_add((d - channel_delta).unsigned_abs());
        v.dif[10] = v.dif[10].wrapping_add((d + channel_delta).unsigned_abs());

        let new_delta = ch.wrapping_sub(v.last_char as u32) as u8 as i8;
        v.last_delta = new_delta as i32;
        self.unp_channel_delta = new_delta as i32;
        v.last_char = ch as i32;

        if v.byte_count & 0x1F == 0 {
            let mut min_dif = v.dif[0];
            let mut num_min_dif = 0usize;
            v.dif[0] = 0;
            for (i, dv) in v.dif.iter_mut().enumerate().skip(1) {
                if *dv < min_dif {
                    min_dif = *dv;
                    num_min_dif = i;
                }
                *dv = 0;
            }
            let v = &mut self.aud_v[cur];
            match num_min_dif {
                1 => {
                    if v.k1 >= -16 {
                        v.k1 -= 1;
                    }
                }
                2 => {
                    if v.k1 < 16 {
                        v.k1 += 1;
                    }
                }
                3 => {
                    if v.k2 >= -16 {
                        v.k2 -= 1;
                    }
                }
                4 => {
                    if v.k2 < 16 {
                        v.k2 += 1;
                    }
                }
                5 => {
                    if v.k3 >= -16 {
                        v.k3 -= 1;
                    }
                }
                6 => {
                    if v.k3 < 16 {
                        v.k3 += 1;
                    }
                }
                7 => {
                    if v.k4 >= -16 {
                        v.k4 -= 1;
                    }
                }
                8 => {
                    if v.k4 < 16 {
                        v.k4 += 1;
                    }
                }
                9 => {
                    if v.k5 >= -16 {
                        v.k5 -= 1;
                    }
                }
                10 => {
                    if v.k5 < 16 {
                        v.k5 += 1;
                    }
                }
                _ => {}
            }
        }
        ch as u8
    }

    // -- v15 ------------------------------------------------------

    const START_L1: u32 = 2;
    const START_L2: u32 = 3;
    const START_HF0: u32 = 4;
    const START_HF1: u32 = 5;
    const START_HF2: u32 = 5;
    const START_HF3: u32 = 6;
    const START_HF4: u32 = 8;

    fn decode_num(&mut self, num: u32, start_pos: u32, dec_tab: &[u32], pos_tab: &[u32]) -> u32 {
        let num = num & 0xFFF0;
        let mut start_pos = start_pos;
        let mut i = 0usize;
        while i < dec_tab.len() && dec_tab[i] <= num {
            start_pos += 1;
            i += 1;
        }
        self.inp.addbits(start_pos);
        let base = if i > 0 { dec_tab[i - 1] } else { 0 };
        ((num.wrapping_sub(base)) >> (16 - start_pos)) + pos_tab[start_pos as usize]
    }

    fn corr_huff(&mut self, which: HuffSet) {
        let (char_set, num_to_place) = self.huff_parts_mut(which);
        let mut idx = 0usize;
        for i in (0..8usize).rev() {
            for _ in 0..32 {
                char_set[idx] = (char_set[idx] & 0xFF00) | i as u16;
                idx += 1;
            }
        }
        *num_to_place = [0u8; 256];
        for (i, v) in num_to_place.iter_mut().enumerate().take(7) {
            *v = ((7 - i) * 32) as u8;
        }
    }

    fn huff_parts_mut(&mut self, which: HuffSet) -> (&mut [u16; 256], &mut [u8; 256]) {
        match which {
            HuffSet::Main => (&mut self.ch_set, &mut self.nto_pl),
            HuffSet::B => (&mut self.ch_set_b, &mut self.nto_pl_b),
            HuffSet::C => (&mut self.ch_set_c, &mut self.nto_pl_c),
        }
    }

    fn init_huff(&mut self) {
        for i in 0..256usize {
            self.ch_set[i] = (i as u16) << 8;
            self.ch_set_b[i] = (i as u16) << 8;
            self.ch_set_a[i] = i as u16;
            self.ch_set_c[i] = ((i as u16).wrapping_neg() & 0xFF) << 8;
        }
        self.nto_pl = [0; 256];
        self.nto_pl_b = [0; 256];
        self.nto_pl_c = [0; 256];
        self.corr_huff(HuffSet::B);
    }

    fn copy_string15(&mut self, distance: u32, length: u32) {
        self.dest_unp_size -= length as i64;
        let mask = self.mask();
        if (!self.first_win_done && distance as usize > self.unp_ptr)
            || distance as usize > self.max_win_size
            || distance == 0
        {
            for _ in 0..length {
                self.window[self.unp_ptr] = 0;
                self.unp_ptr = (self.unp_ptr + 1) & mask;
            }
        } else {
            for _ in 0..length {
                self.window[self.unp_ptr] =
                    self.window[self.unp_ptr.wrapping_sub(distance as usize) & mask];
                self.unp_ptr = (self.unp_ptr + 1) & mask;
            }
        }
    }

    fn unpack15(&mut self, solid: bool) {
        self.inp.unp_read_buf();
        if !solid {
            self.init_huff();
            self.unp_ptr = 0;
        } else {
            self.unp_ptr = self.wr_ptr;
        }
        self.dest_unp_size -= 1;
        if self.dest_unp_size >= 0 {
            self.get_flags_buf();
            self.flags_cnt = 8;
        }

        while self.dest_unp_size >= 0 {
            self.unp_ptr &= self.mask();
            self.first_win_done |= self.prev_ptr > self.unp_ptr;
            self.prev_ptr = self.unp_ptr;

            if self.inp.in_addr as isize > self.inp.read_top as isize - 30
                && !self.inp.unp_read_buf()
            {
                break;
            }
            if self.wr_ptr.wrapping_sub(self.unp_ptr) & self.mask() < 270
                && self.wr_ptr != self.unp_ptr
            {
                self.unp_write_buf20();
            }
            if self.st_mode != 0 {
                self.huff_decode();
                continue;
            }

            self.flags_cnt -= 1;
            if self.flags_cnt < 0 {
                self.get_flags_buf();
                self.flags_cnt = 7;
            }

            if self.flag_buf & 0x80 != 0 {
                self.flag_buf <<= 1;
                if self.nlzb > self.nhfb {
                    self.long_lz();
                } else {
                    self.huff_decode();
                }
            } else {
                self.flag_buf <<= 1;
                self.flags_cnt -= 1;
                if self.flags_cnt < 0 {
                    self.get_flags_buf();
                    self.flags_cnt = 7;
                }
                if self.flag_buf & 0x80 != 0 {
                    self.flag_buf <<= 1;
                    if self.nlzb > self.nhfb {
                        self.huff_decode();
                    } else {
                        self.long_lz();
                    }
                } else {
                    self.flag_buf <<= 1;
                    self.short_lz();
                }
            }
        }
        self.unp_write_buf20();
    }

    fn get_flags_buf(&mut self) {
        let flags_place = self.decode_num(self.inp.getbits(), Self::START_HF2, &DEC_HF2, &POS_HF2);
        if flags_place as usize >= 256 {
            return;
        }
        let flags_place = flags_place as usize;
        loop {
            let flags = self.ch_set_c[flags_place];
            self.flag_buf = u32::from(flags >> 8);
            let idx = (flags & 0xFF) as usize;
            let new_flags_place = self.nto_pl_c[idx] as usize;
            self.nto_pl_c[idx] += 1;
            let flags_inc = flags.wrapping_add(1);
            if flags_inc & 0xFF != 0 {
                let a = self.ch_set_c[new_flags_place];
                self.ch_set_c[flags_place] = a;
                self.ch_set_c[new_flags_place] = flags_inc;
                break;
            }
            self.corr_huff(HuffSet::C);
        }
    }

    fn huff_decode(&mut self) {
        let bit_field = self.inp.getbits();

        let byte_place = if self.avr_plc > 0x75FF {
            self.decode_num(bit_field, Self::START_HF4, &DEC_HF4, &POS_HF4) as i32
        } else if self.avr_plc > 0x5DFF {
            self.decode_num(bit_field, Self::START_HF3, &DEC_HF3, &POS_HF3) as i32
        } else if self.avr_plc > 0x35FF {
            self.decode_num(bit_field, Self::START_HF2, &DEC_HF2, &POS_HF2) as i32
        } else if self.avr_plc > 0x0DFF {
            self.decode_num(bit_field, Self::START_HF1, &DEC_HF1, &POS_HF1) as i32
        } else {
            self.decode_num(bit_field, Self::START_HF0, &DEC_HF0, &POS_HF0) as i32
        };
        let mut byte_place = byte_place & 0xFF;
        if self.st_mode != 0 {
            if byte_place == 0 && bit_field > 0xFFF {
                byte_place = 0x100;
            }
            byte_place -= 1;
            if byte_place == -1 {
                let bit_field = self.inp.getbits();
                self.inp.addbits(1);
                if bit_field & 0x8000 != 0 {
                    self.num_huf = 0;
                    self.st_mode = 0;
                    return;
                }
                let length = if bit_field & 0x4000 != 0 { 4 } else { 3 };
                self.inp.addbits(1);
                let distance = self.decode_num(self.inp.getbits(), Self::START_HF2, &DEC_HF2, &POS_HF2);
                let distance = (distance << 5) | (self.inp.getbits() >> 11);
                self.inp.addbits(5);
                self.copy_string15(distance, length);
                return;
            }
        } else {
            self.num_huf += 1;
            if self.num_huf >= 16 && self.flags_cnt == 0 {
                self.st_mode = 1;
            }
        }
        let byte_place = byte_place as usize;
        self.avr_plc = self.avr_plc.wrapping_add(byte_place as u32);
        self.avr_plc = self.avr_plc.wrapping_sub(self.avr_plc >> 8);
        self.nhfb += 16;
        if self.nhfb > 0xFF {
            self.nhfb = 0x90;
            self.nlzb >>= 1;
        }

        self.window[self.unp_ptr] = (self.ch_set[byte_place] >> 8) as u8;
        self.unp_ptr += 1;
        self.dest_unp_size -= 1;

        loop {
            let cur_byte = self.ch_set[byte_place];
            let cur_byte_inc = cur_byte.wrapping_add(1);
            let idx = (cur_byte & 0xFF) as usize;
            let new_byte_place = self.nto_pl[idx] as usize;
            self.nto_pl[idx] += 1;
            if cur_byte_inc & 0xFF > 0xA1 {
                self.corr_huff(HuffSet::Main);
            } else {
                let a = self.ch_set[new_byte_place];
                self.ch_set[byte_place] = a;
                self.ch_set[new_byte_place] = cur_byte_inc;
                break;
            }
        }
    }

    fn short_lz(&mut self) {
        static SHORT_XOR1: [u32; 15] = [
            0, 0xA0, 0xD0, 0xE0, 0xF0, 0xF8, 0xFC, 0xFE, 0xFF, 0xC0, 0x80, 0x90, 0x98, 0x9C,
            0xB0,
        ];
        static SHORT_XOR2: [u32; 15] = [
            0, 0x40, 0x60, 0xA0, 0xD0, 0xE0, 0xF0, 0xF8, 0xFC, 0xC0, 0x80, 0x90, 0x98, 0x9C,
            0xB0,
        ];

        self.num_huf = 0;
        let mut bit_field = self.inp.getbits();
        if self.l_count == 2 {
            self.inp.addbits(1);
            if bit_field >= 0x8000 {
                let d = self.last_dist;
                let l = self.last_length;
                self.copy_string15(d, l);
                return;
            }
            bit_field <<= 1;
            self.l_count = 0;
        }

        let bit_field = bit_field >> 8;
        let mut length = 0u32;
        if self.avr_ln1 < 37 {
            loop {
                if length as usize >= SHORT_XOR1.len() {
                    return; // corrupt: no matching code
                }
                let sl = Self::get_short_len1(length, self.buf60);
                if (bit_field ^ SHORT_XOR1[length as usize]) & (!(!0u32 >> sl)) == 0 {
                    break;
                }
                length += 1;
            }
            self.inp
                .addbits(Self::get_short_len1(length, self.buf60));
        } else {
            loop {
                if length as usize >= SHORT_XOR2.len() {
                    return; // corrupt: no matching code
                }
                let sl = Self::get_short_len2(length, self.buf60);
                if (bit_field ^ SHORT_XOR2[length as usize]) & (!(!0u32 >> sl)) == 0 {
                    break;
                }
                length += 1;
            }
            self.inp
                .addbits(Self::get_short_len2(length, self.buf60));
        }

        if length >= 9 {
            if length == 9 {
                self.l_count += 1;
                let d = self.last_dist;
                let l = self.last_length;
                self.copy_string15(d, l);
                return;
            }
            if length == 14 {
                self.l_count = 0;
                let length = self.decode_num(self.inp.getbits(), Self::START_L2, &DEC_L2, &POS_L2) + 5;
                let distance = (self.inp.getbits() >> 1) | 0x8000;
                self.inp.addbits(15);
                self.last_length = length;
                self.last_dist = distance;
                self.copy_string15(distance, length);
                return;
            }

            self.l_count = 0;
            let save_length = length;
            let distance = self.old_dist[(self.old_dist_ptr + 4)
                .wrapping_sub(length as usize - 9)
                & 3] as u32;
            let length =
                self.decode_num(self.inp.getbits(), Self::START_L1, &DEC_L1, &POS_L1) + 2;
            if length == 0x101 && save_length == 10 {
                self.buf60 ^= 1;
                return;
            }
            let mut length = length;
            if distance > 256 {
                length += 1;
            }
            if distance >= self.max_dist3 {
                length += 1;
            }
            self.old_dist[self.old_dist_ptr] = distance as usize;
            self.old_dist_ptr += 1;
            self.old_dist_ptr &= 3;
            self.last_length = length;
            self.last_dist = distance;
            self.copy_string15(distance, length);
            return;
        }

        self.l_count = 0;
        self.avr_ln1 += length;
        self.avr_ln1 -= self.avr_ln1 >> 4;

        let distance_place =
            self.decode_num(self.inp.getbits(), Self::START_HF2, &DEC_HF2, &POS_HF2) & 0xFF;
        let distance = u32::from(self.ch_set_a[distance_place as usize]);
        let distance_place = distance_place as i32 - 1;
        if distance_place != -1 {
            let dp = distance_place as usize;
            let last_distance = self.ch_set_a[dp];
            self.ch_set_a[dp + 1] = last_distance;
            self.ch_set_a[dp] = distance as u16;
        }
        let length = length + 2;
        let distance = distance.wrapping_add(1);
        self.old_dist[self.old_dist_ptr] = distance as usize;
        self.old_dist_ptr += 1;
        self.old_dist_ptr &= 3;
        self.last_length = length;
        self.last_dist = distance;
        self.copy_string15(distance, length);
    }

    fn get_short_len1(pos: u32, buf60: i32) -> u32 {
        if pos == 1 {
            (buf60 + 3) as u32
        } else {
            SHORT_LEN1[pos as usize]
        }
    }
    fn get_short_len2(pos: u32, buf60: i32) -> u32 {
        if pos == 3 {
            (buf60 + 3) as u32
        } else {
            SHORT_LEN2[pos as usize]
        }
    }

    fn long_lz(&mut self) {
        self.num_huf = 0;
        self.nlzb += 16;
        if self.nlzb > 0xFF {
            self.nlzb = 0x90;
            self.nhfb >>= 1;
        }
        let old_avr2 = self.avr_ln2;

        let bit_field = self.inp.getbits();
        let length;
        if self.avr_ln2 >= 122 {
            length = self.decode_num(bit_field, Self::START_L2, &DEC_L2, &POS_L2);
        } else if self.avr_ln2 >= 64 {
            length = self.decode_num(bit_field, Self::START_L1, &DEC_L1, &POS_L1);
        } else if bit_field < 0x100 {
            length = bit_field;
            self.inp.addbits(16);
        } else {
            let mut l = 0u32;
            while (bit_field << l) & 0x8000 == 0 {
                l += 1;
            }
            length = l;
            self.inp.addbits(l + 1);
        }

        self.avr_ln2 = self.avr_ln2.wrapping_add(length);
        self.avr_ln2 -= self.avr_ln2 >> 5;

        let bit_field = self.inp.getbits();
        let distance_place;
        if self.avr_plc_b > 0x28FF {
            distance_place = self.decode_num(bit_field, Self::START_HF2, &DEC_HF2, &POS_HF2);
        } else if self.avr_plc_b > 0x6FF {
            distance_place = self.decode_num(bit_field, Self::START_HF1, &DEC_HF1, &POS_HF1);
        } else {
            distance_place = self.decode_num(bit_field, Self::START_HF0, &DEC_HF0, &POS_HF0);
        }

        self.avr_plc_b = self.avr_plc_b.wrapping_add(distance_place);
        self.avr_plc_b -= self.avr_plc_b >> 8;

        let distance_place = (distance_place & 0xFF) as usize;
        let distance;
        loop {
            let d = self.ch_set_b[distance_place];
            let idx = (d & 0xFF) as usize;
            let new_distance_place = self.nto_pl_b[idx] as usize;
            self.nto_pl_b[idx] += 1;
            let d_inc = d.wrapping_add(1);
            if d_inc & 0xFF != 0 {
                let b = self.ch_set_b[new_distance_place];
                self.ch_set_b[distance_place] = b;
                self.ch_set_b[new_distance_place] = d_inc;
                distance = u32::from(d_inc);
                break;
            }
            self.corr_huff(HuffSet::B);
        }

        let distance = ((distance & 0xFF00) | (self.inp.getbits() >> 8)) >> 1;
        self.inp.addbits(7);

        let old_avr3 = self.avr_ln3;
        if length != 1 && length != 4 {
            if length == 0 && distance <= self.max_dist3 {
                self.avr_ln3 += 1;
                self.avr_ln3 -= self.avr_ln3 >> 8;
            } else if self.avr_ln3 > 0 {
                self.avr_ln3 -= 1;
            }
        }
        let mut length = length + 3;
        if distance >= self.max_dist3 {
            length += 1;
        }
        if distance <= 256 {
            length += 8;
        }
        if old_avr3 > 0xB0 || self.avr_plc >= 0x2A00 && old_avr2 < 0x40 {
            self.max_dist3 = 0x7F00;
        } else {
            self.max_dist3 = 0x2001;
        }
        self.old_dist[self.old_dist_ptr] = distance as usize;
        self.old_dist_ptr += 1;
        self.old_dist_ptr &= 3;
        self.last_length = length;
        self.last_dist = distance;
        self.copy_string15(distance, length);
    }
}

#[derive(Clone, Copy)]
enum HuffSet {
    Main,
    B,
    C,
}

impl Default for Unpacker30 {
    fn default() -> Self {
        Self::new()
    }
}
