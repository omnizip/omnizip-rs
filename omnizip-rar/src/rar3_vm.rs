//! RAR virtual machine for RAR3 filters — port of unrar's rarvm.cpp.
//! Modern unrar executes only the six known standard filter programs
//! (E8, E8E9, Itanium, Delta, RGB, Audio), matched by code length +
//! CRC32; arbitrary VM bytecode is not executed (its output blocks
//! are dropped, exactly like the reference).
#![forbid(unsafe_code)]

use crate::rar3_unpack::InBuf30;

pub const VM_MEMSIZE: usize = 0x40000;
const VM_MEMMASK: usize = VM_MEMSIZE - 1;
const MAX3_UNPACK_CHANNELS: u32 = 1024;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum VmFilter {
    #[default]
    None,
    E8,
    E8e9,
    Itanium,
    Rgb,
    Audio,
    Delta,
}

/// `RarVM::ReadData`: variable-width value from the bit stream.
pub fn read_data(inp: &mut InBuf30) -> u32 {
    let data = inp.getbits();
    match data & 0xC000 {
        0x0000 => {
            inp.addbits(6);
            (data >> 10) & 0xF
        }
        0x4000 => {
            if data & 0x3C00 == 0 {
                let v = 0xFFFF_FF00 | ((data >> 2) & 0xFF);
                inp.addbits(14);
                v
            } else {
                let v = (data >> 6) & 0xFF;
                inp.addbits(10);
                v
            }
        }
        0x8000 => {
            inp.addbits(2);
            let v = inp.getbits();
            inp.addbits(16);
            v
        }
        _ => {
            inp.addbits(2);
            let mut v = inp.getbits() << 16;
            inp.addbits(16);
            v |= inp.getbits();
            inp.addbits(16);
            v
        }
    }
}

/// `RarVM::Prepare`: identify the standard filter (checksum + CRC).
pub fn prepare(code: &[u8]) -> VmFilter {
    let mut xor_sum = 0u8;
    for &b in &code[1..] {
        xor_sum ^= b;
    }
    if xor_sum != code[0] {
        return VmFilter::None;
    }
    const STD_LIST: [(u32, u32, VmFilter); 6] = [
        (53, 0xAD57_6887, VmFilter::E8),
        (57, 0x3CD7_E57E, VmFilter::E8e9),
        (120, 0x3769_893F, VmFilter::Itanium),
        (29, 0x0E06_077D, VmFilter::Delta),
        (149, 0x1C2C_5DC8, VmFilter::Rgb),
        (216, 0xBC85_E701, VmFilter::Audio),
    ];
    let code_crc = omnizip_archive_core::crc32(code);
    for (len, crc, kind) in STD_LIST {
        if crc == code_crc && len as usize == code.len() {
            return kind;
        }
    }
    VmFilter::None
}

/// Result of `RarVM::Execute`: the window-relative output span.
pub struct VmOutput {
    pub offset: usize,
    pub size: usize,
}

/// The VM's 256 KiB data memory.
pub struct RarVm {
    pub mem: Vec<u8>,
}

impl RarVm {
    #[must_use]
    pub fn new() -> Self {
        Self {
            mem: vec![0u8; VM_MEMSIZE + 4],
        }
    }

    /// `RarVM::SetMemory`.
    pub fn set_memory(&mut self, pos: usize, data: &[u8]) {
        if pos < VM_MEMSIZE {
            let copy = data.len().min(VM_MEMSIZE - pos);
            // Copy through an owned buffer: the source can alias our
            // own memory (window data staged at another offset).
            let src = data[..copy].to_vec();
            self.mem[pos..pos + copy].copy_from_slice(&src);
        }
    }

    /// `RarVM::Execute` for a prepared program. Returns the filtered
    /// span in VM memory (already deinterleaved for delta/rgb/audio).
    pub fn execute(&mut self, filter: VmFilter, init_r: &[u32; 7]) -> VmOutput {
        let mut r = [0u32; 8];
        r[..7].copy_from_slice(init_r);
        if filter == VmFilter::None {
            return VmOutput { offset: 0, size: 0 };
        }
        let success = self.execute_standard_filter(filter, &r);
        let block_size = (r[4] as usize) & VM_MEMMASK;
        let offset = match filter {
            VmFilter::Delta | VmFilter::Rgb | VmFilter::Audio => {
                if 2 * block_size > VM_MEMSIZE || !success {
                    0
                } else {
                    block_size
                }
            }
            _ => 0,
        };
        VmOutput {
            offset,
            size: block_size,
        }
    }

    fn execute_standard_filter(&mut self, filter: VmFilter, r: &[u32; 8]) -> bool {
        match filter {
            VmFilter::E8 | VmFilter::E8e9 => {
                let data_size = r[4];
                let file_offset = r[6];
                if data_size as usize > VM_MEMSIZE || data_size < 4 {
                    return false;
                }
                const FILE_SIZE: u32 = 0x100_0000;
                let cmp_byte2: u8 = if filter == VmFilter::E8e9 { 0xE9 } else { 0xE8 };
                let mut cur_pos = 0usize;
                let mut ptr = 0usize;
                while cur_pos < data_size as usize - 4 {
                    let cur_byte = self.mem[ptr];
                    ptr += 1;
                    cur_pos += 1;
                    if cur_byte == 0xE8 || cur_byte == cmp_byte2 {
                        let offset = (cur_pos as u32).wrapping_add(file_offset);
                        let addr =
                            u32::from_le_bytes(self.mem[ptr..ptr + 4].try_into().expect("4"));
                        if addr & 0x8000_0000 != 0 {
                            if addr.wrapping_add(offset) & 0x8000_0000 == 0 {
                                self.mem[ptr..ptr + 4]
                                    .copy_from_slice(&addr.wrapping_add(FILE_SIZE).to_le_bytes());
                            }
                        } else if addr.wrapping_sub(FILE_SIZE) & 0x8000_0000 != 0 {
                            self.mem[ptr..ptr + 4]
                                .copy_from_slice(&addr.wrapping_sub(offset).to_le_bytes());
                        }
                        ptr += 4;
                        cur_pos += 4;
                    }
                }
                true
            }
            VmFilter::Itanium => {
                let data_size = r[4];
                let mut file_offset = r[6];
                if data_size as usize > VM_MEMSIZE || data_size < 21 {
                    return false;
                }
                let mut cur_pos = 0usize;
                let mut ptr = 0usize;
                file_offset >>= 4;
                while cur_pos < data_size as usize - 21 {
                    let byte = (self.mem[ptr] & 0x1F) as i32 - 0x10;
                    if byte >= 0 {
                        const MASKS: [u8; 16] = [4, 4, 6, 6, 0, 0, 7, 7, 4, 4, 0, 0, 4, 4, 0, 0];
                        let cmd_mask = MASKS[byte as usize];
                        if cmd_mask != 0 {
                            for i in 0..3u32 {
                                if cmd_mask & (1 << i) != 0 {
                                    let start_pos = i * 41 + 5;
                                    let op_type = self.itanium_get_bits(ptr, start_pos + 37, 4);
                                    if op_type == 5 {
                                        let offset = self.itanium_get_bits(ptr, start_pos + 13, 20);
                                        self.itanium_set_bits(
                                            ptr,
                                            offset.wrapping_sub(file_offset) & 0xF_FFFF,
                                            start_pos + 13,
                                            20,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    ptr += 16;
                    cur_pos += 16;
                    file_offset = file_offset.wrapping_add(1);
                }
                true
            }
            VmFilter::Delta => {
                let data_size = r[4];
                let channels = r[0];
                if data_size as usize > VM_MEMSIZE / 2
                    || channels > MAX3_UNPACK_CHANNELS
                    || channels == 0
                {
                    return false;
                }
                let border = data_size as usize * 2;
                let mut src_pos = 0usize;
                for cur_channel in 0..channels as usize {
                    let mut prev_byte: u8 = 0;
                    let mut dest_pos = data_size as usize + cur_channel;
                    while dest_pos < border {
                        prev_byte = prev_byte.wrapping_sub(self.mem[src_pos]);
                        self.mem[dest_pos] = prev_byte;
                        src_pos += 1;
                        dest_pos += channels as usize;
                    }
                }
                true
            }
            VmFilter::Rgb => {
                let data_size = r[4];
                let width = r[0].wrapping_sub(3);
                let pos_r = r[1];
                if data_size as usize > VM_MEMSIZE / 2
                    || data_size < 3
                    || width > data_size
                    || pos_r > 2
                {
                    return false;
                }
                let data_size = data_size as usize;
                let width = width as usize;
                let src_base = 0usize;
                let dest_base = data_size;
                for cur_channel in 0..3usize {
                    let mut prev_byte = 0u32;
                    let mut i = cur_channel;
                    let mut src = src_base;
                    while i < data_size {
                        let predicted = if i >= width + 3 {
                            let upper = u32::from(self.mem[dest_base + i - width]);
                            let upper_left = u32::from(self.mem[dest_base + i - width - 3]);
                            let p = prev_byte + upper - upper_left;
                            let pa = (p as i32 - prev_byte as i32).unsigned_abs();
                            let pb = (p as i32 - upper as i32).unsigned_abs();
                            let pc = (p as i32 - upper_left as i32).unsigned_abs();
                            if pa <= pb && pa <= pc {
                                prev_byte
                            } else if pb <= pc {
                                upper
                            } else {
                                upper_left
                            }
                        } else {
                            prev_byte
                        };
                        prev_byte = predicted.wrapping_sub(u32::from(self.mem[src]));
                        self.mem[dest_base + i] = prev_byte as u8;
                        src += 1;
                        i += 3;
                    }
                }
                let mut i = pos_r as usize;
                let border = data_size - 2;
                while i < border {
                    let g = self.mem[dest_base + i + 1];
                    self.mem[dest_base + i] = self.mem[dest_base + i].wrapping_add(g);
                    self.mem[dest_base + i + 2] = self.mem[dest_base + i + 2].wrapping_add(g);
                    i += 3;
                }
                true
            }
            VmFilter::Audio => {
                let data_size = r[4];
                let channels = r[0];
                if data_size as usize > VM_MEMSIZE / 2 || channels > 128 || channels == 0 {
                    return false;
                }
                let data_size = data_size as usize;
                for cur_channel in 0..channels as usize {
                    let mut prev_byte = 0u32;
                    let mut prev_delta = 0i32;
                    let mut dif = [0u32; 7];
                    let mut d1 = 0i32;
                    let mut d2 = 0i32;
                    let mut d3;
                    let mut k1 = 0i32;
                    let mut k2 = 0i32;
                    let mut k3 = 0i32;
                    let mut src = 0usize;
                    let mut byte_count = 0u32;
                    let mut i = cur_channel;
                    while i < data_size {
                        d3 = d2;
                        d2 = prev_delta - d1;
                        d1 = prev_delta;

                        let mut predicted = 8 * prev_byte as i32 + k1 * d1 + k2 * d2 + k3 * d3;
                        predicted = (predicted >> 3) & 0xFF;

                        let cur_byte = self.mem[src];
                        src += 1;

                        predicted -= i32::from(cur_byte);
                        self.mem[data_size + i] = predicted as u8;
                        prev_delta = ((predicted - prev_byte as i32) as i8) as i32;
                        prev_byte = predicted as u32;

                        let d = (cur_byte as i8 as i32) << 3;

                        dif[0] = dif[0].wrapping_add(d.unsigned_abs());
                        dif[1] = dif[1].wrapping_add((d - d1).unsigned_abs());
                        dif[2] = dif[2].wrapping_add((d + d1).unsigned_abs());
                        dif[3] = dif[3].wrapping_add((d - d2).unsigned_abs());
                        dif[4] = dif[4].wrapping_add((d + d2).unsigned_abs());
                        dif[5] = dif[5].wrapping_add((d - d3).unsigned_abs());
                        dif[6] = dif[6].wrapping_add((d + d3).unsigned_abs());

                        if byte_count & 0x1F == 0 {
                            let mut min_dif = dif[0];
                            let mut num_min_dif = 0usize;
                            dif[0] = 0;
                            for (j, v) in dif.iter_mut().enumerate().skip(1) {
                                if *v < min_dif {
                                    min_dif = *v;
                                    num_min_dif = j;
                                }
                                *v = 0;
                            }
                            match num_min_dif {
                                1 => {
                                    if k1 >= -16 {
                                        k1 -= 1;
                                    }
                                }
                                2 => {
                                    if k1 < 16 {
                                        k1 += 1;
                                    }
                                }
                                3 => {
                                    if k2 >= -16 {
                                        k2 -= 1;
                                    }
                                }
                                4 => {
                                    if k2 < 16 {
                                        k2 += 1;
                                    }
                                }
                                5 => {
                                    if k3 >= -16 {
                                        k3 -= 1;
                                    }
                                }
                                6 => {
                                    if k3 < 16 {
                                        k3 += 1;
                                    }
                                }
                                _ => {}
                            }
                        }
                        byte_count += 1;
                        i += channels as usize;
                    }
                }
                true
            }
            VmFilter::None => true,
        }
    }

    fn itanium_get_bits(&self, base: usize, bit_pos: u32, bit_count: u32) -> u32 {
        let in_addr = base + (bit_pos >> 3) as usize;
        let in_bit = bit_pos & 7;
        let mut bit_field = u32::from(self.mem[in_addr]);
        bit_field |= u32::from(self.mem[in_addr + 1]) << 8;
        bit_field |= u32::from(self.mem[in_addr + 2]) << 16;
        bit_field |= u32::from(self.mem[in_addr + 3]) << 24;
        bit_field >>= in_bit;
        bit_field & (0xFFFF_FFFFu32 >> (32 - bit_count))
    }

    fn itanium_set_bits(&mut self, base: usize, bit_field: u32, bit_pos: u32, bit_count: u32) {
        let in_addr = base + (bit_pos >> 3) as usize;
        let in_bit = bit_pos & 7;
        let mut and_mask = 0xFFFF_FFFFu32 >> (32 - bit_count);
        and_mask = !(and_mask << in_bit);
        let mut bit_field = bit_field << in_bit;
        for i in 0..4 {
            self.mem[in_addr + i] &= and_mask as u8;
            self.mem[in_addr + i] |= bit_field as u8;
            and_mask = (and_mask >> 8) | 0xFF00_0000;
            bit_field >>= 8;
        }
    }
}

impl Default for RarVm {
    fn default() -> Self {
        Self::new()
    }
}
