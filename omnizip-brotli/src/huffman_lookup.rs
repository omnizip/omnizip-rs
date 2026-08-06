//! Table-based Huffman decoder — ported from upstream
//! `brotli-decompressor/src/huffman/mod.rs` (BSD-3-Clause).
//!
//! This replaces the bit-by-bit walker in `decoder.rs::HuffmanTable`
//! with the proven correct, fast table-lookup approach. The upstream
//! implementation is the reference for all brotli decoders; porting
//! it directly eliminates the correctness bugs in the from-scratch
//! bit-by-bit walker.

#![forbid(unsafe_code)]

/// A single entry in a Huffman lookup table.
///
/// For a root-level entry: `value` = decoded symbol, `bits` = number
/// of bits consumed from the bitstream.
///
/// For a pointer to a 2nd-level sub-table: `bits` = root_bits +
/// sub_table_bits, `value` = offset from this entry to the sub-table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HuffmanCode {
    pub value: u16,
    pub bits: u8,
}

/// Maximum Huffman code length (RFC 7932 §9.5).
pub const MAX_CODE_LENGTH: usize = 15;
/// Root table bits: the number of bits peeked in the first-level lookup.
pub const ROOT_BITS: u32 = 8;
/// Maximum table size for the worst-case alphabet.
pub const MAX_TABLE_SIZE: usize = 1080;

/// 256-entry bit-reversal lookup table (reverses 8 bits).
const REVERSE_BITS: [u8; 256] = [
    0x00, 0x80, 0x40, 0xC0, 0x20, 0xA0, 0x60, 0xE0, 0x10, 0x90, 0x50, 0xD0, 0x30, 0xB0, 0x70, 0xF0,
    0x08, 0x88, 0x48, 0xC8, 0x28, 0xA8, 0x68, 0xE8, 0x18, 0x98, 0x58, 0xD8, 0x38, 0xB8, 0x78, 0xF8,
    0x04, 0x84, 0x44, 0xC4, 0x24, 0xA4, 0x64, 0xE4, 0x14, 0x94, 0x54, 0xD4, 0x34, 0xB4, 0x74, 0xF4,
    0x0C, 0x8C, 0x4C, 0xCC, 0x2C, 0xAC, 0x6C, 0xEC, 0x1C, 0x9C, 0x5C, 0xDC, 0x3C, 0xBC, 0x7C, 0xFC,
    0x02, 0x82, 0x42, 0xC2, 0x22, 0xA2, 0x62, 0xE2, 0x12, 0x92, 0x52, 0xD2, 0x32, 0xB2, 0x72, 0xF2,
    0x0A, 0x8A, 0x4A, 0xCA, 0x2A, 0xAA, 0x6A, 0xEA, 0x1A, 0x9A, 0x5A, 0xDA, 0x3A, 0xBA, 0x7A, 0xFA,
    0x06, 0x86, 0x46, 0xC6, 0x26, 0xA6, 0x66, 0xE6, 0x16, 0x96, 0x56, 0xD6, 0x36, 0xB6, 0x76, 0xF6,
    0x0E, 0x8E, 0x4E, 0xCE, 0x2E, 0xAE, 0x6E, 0xEE, 0x1E, 0x9E, 0x5E, 0xDE, 0x3E, 0xBE, 0x7E, 0xFE,
    0x01, 0x81, 0x41, 0xC1, 0x21, 0xA1, 0x61, 0xE1, 0x11, 0x91, 0x51, 0xD1, 0x31, 0xB1, 0x71, 0xF1,
    0x09, 0x89, 0x49, 0xC9, 0x29, 0xA9, 0x69, 0xE9, 0x19, 0x99, 0x59, 0xD9, 0x39, 0xB9, 0x79, 0xF9,
    0x05, 0x85, 0x45, 0xC5, 0x25, 0xA5, 0x65, 0xE5, 0x15, 0x95, 0x55, 0xD5, 0x35, 0xB5, 0x75, 0xF5,
    0x0D, 0x8D, 0x4D, 0xCD, 0x2D, 0xAD, 0x6D, 0xED, 0x1D, 0x9D, 0x5D, 0xDD, 0x3D, 0xBD, 0x7D, 0xFD,
    0x03, 0x83, 0x43, 0xC3, 0x23, 0xA3, 0x63, 0xE3, 0x13, 0x93, 0x53, 0xD3, 0x33, 0xB3, 0x73, 0xF3,
    0x0B, 0x8B, 0x4B, 0xCB, 0x2B, 0xAB, 0x6B, 0xEB, 0x1B, 0x9B, 0x5B, 0xDB, 0x3B, 0xBB, 0x7B, 0xFB,
    0x07, 0x87, 0x47, 0xC7, 0x27, 0xA7, 0x67, 0xE7, 0x17, 0x97, 0x57, 0xD7, 0x37, 0xB7, 0x77, 0xF7,
    0x0F, 0x8F, 0x4F, 0xCF, 0x2F, 0xAF, 0x6F, 0xEF, 0x1F, 0x9F, 0x5F, 0xDF, 0x3F, 0xBF, 0x7F, 0xFF,
];

const REVERSE_BITS_LOWEST: u32 = 0x80;

/// Build a flat Huffman lookup table from per-symbol code lengths.
///
/// Returns the total table size (root table + any 2nd-level sub-tables).
///
/// Ported from upstream `BrotliBuildHuffmanTable`. Uses `symbol_lists`
/// format where symbols are grouped by code length, sorted within each
/// length. `count[i]` = number of symbols with code length `i`.
pub fn build_huffman_table(
    root_table: &mut [HuffmanCode],
    symbol_lists: &[u16],
    symbol_lists_offset: usize,
    count: &mut [u16],
) -> u32 {
    let mut code = HuffmanCode { bits: 0, value: 0 };
    let mut max_length: i32 = -1;

    // Find the maximum code length.
    while symbol_lists[symbol_lists_offset.wrapping_add_signed(max_length as isize)] == 0xFFFF {
        max_length -= 1;
    }
    max_length += MAX_CODE_LENGTH as i32 + 1;

    let mut table_offset: u32 = 0;
    let mut table_bits: i32 = ROOT_BITS as i32;
    let mut table_size: i32 = 1 << table_bits;
    let mut total_size: i32 = table_size;

    // Reduce root table if possible.
    if table_bits > max_length {
        table_bits = max_length;
        table_size = 1 << table_bits;
    }

    // Fill root table.
    let mut key: u32 = 0;
    let mut key_step: u32 = REVERSE_BITS_LOWEST;
    let mut bits: i32 = 1;
    let mut step: i32 = 2;
    loop {
        code.bits = bits as u8;
        let mut symbol: i32 = bits - (MAX_CODE_LENGTH as i32 + 1);
        let mut bits_count: i32 = count[bits as usize] as i32;
        while bits_count != 0 {
            symbol = symbol_lists[symbol_lists_offset.wrapping_add_signed(symbol as isize)] as i32;
            code.value = symbol as u16;
            replicate_value(root_table, table_offset + reverse_bits(key), step, table_size, code);
            key += key_step;
            bits_count -= 1;
        }
        step <<= 1;
        key_step >>= 1;
        bits += 1;
        if !(bits <= table_bits) {
            break;
        }
    }

    // Replicate root table to fill ROOT_BITS entries.
    while total_size != table_size {
        for index in 0..table_size {
            root_table[table_offset as usize + table_size as usize + index as usize] =
                root_table[table_offset as usize + index as usize];
        }
        table_size <<= 1;
    }

    // Fill 2nd level tables.
    key_step = REVERSE_BITS_LOWEST >> (ROOT_BITS as i32 - 1);
    let mut sub_key: u32 = REVERSE_BITS_LOWEST << 1;
    let mut sub_key_step: u32 = REVERSE_BITS_LOWEST;
    step = 2;

    let mut len: i32 = ROOT_BITS as i32 + 1;
    while len <= max_length {
        let mut symbol: i32 = len - (MAX_CODE_LENGTH as i32 + 1);
        while count[len as usize] != 0 {
            if sub_key == (REVERSE_BITS_LOWEST << 1) {
                table_offset += table_size as u32;
                table_bits = next_table_bit_size(count, len, ROOT_BITS as i32);
                table_size = 1 << table_bits;
                total_size += table_size;
                sub_key = reverse_bits(key);
                key += key_step;
                root_table[sub_key as usize].bits = (table_bits + ROOT_BITS as i32) as u8;
                root_table[sub_key as usize].value =
                    (table_offset as usize - sub_key as usize) as u16;
                sub_key = 0;
            }
            code.bits = (len - ROOT_BITS as i32) as u8;
            symbol = symbol_lists[symbol_lists_offset.wrapping_add_signed(symbol as isize)] as i32;
            code.value = symbol as u16;
            replicate_value(root_table, table_offset + reverse_bits(sub_key), step, table_size, code);
            sub_key += sub_key_step;
            count[len as usize] -= 1;
        }
        step <<= 1;
        sub_key_step >>= 1;
        len += 1;
    }
    total_size as u32
}

/// Build a simple-form Huffman table (NSYM ≤ 4 symbols).
pub fn build_simple_huffman_table(
    table: &mut [HuffmanCode],
    val: &[u16],
    num_symbols: u32,
) -> u32 {
    let goal_size = 1u32 << ROOT_BITS;
    let mut table_size: u32 = 1;

    match num_symbols {
        0 => {
            table[0].bits = 0;
            table[0].value = val[0];
        }
        1 => {
            table[0].bits = 1;
            table[1].bits = 1;
            if val[1] > val[0] {
                table[0].value = val[0];
                table[1].value = val[1];
            } else {
                table[0].value = val[1];
                table[1].value = val[0];
            }
            table_size = 2;
        }
        2 => {
            table[0].bits = 1;
            table[0].value = val[0];
            table[2].bits = 1;
            table[2].value = val[0];
            if val[2] > val[1] {
                table[1].value = val[1];
                table[3].value = val[2];
            } else {
                table[1].value = val[2];
                table[3].value = val[1];
            }
            table[1].bits = 2;
            table[3].bits = 2;
            table_size = 4;
        }
        3 => {
            let last: u16 = if val.len() > 3 { val[3] } else { 65535 };
            let mut mval = [val[0], val[1], val[2], last];
            for i in 0..3 {
                for k in (i + 1)..4 {
                    if mval[k] < mval[i] {
                        mval.swap(k, i);
                    }
                }
            }
            for entry in table.iter_mut().take(4) {
                entry.bits = 2;
            }
            table[0].value = mval[0];
            table[2].value = mval[1];
            table[1].value = mval[2];
            table[3].value = mval[3];
            table_size = 4;
        }
        4 => {
            let mut mval = [val[0], val[1], val[2], val[3]];
            if mval[3] < mval[2] {
                mval.swap(3, 2);
            }
            for i in 0..8 {
                table[i].value = mval[0];
                table[i].bits = (1 + (i & 1)) as u8;
            }
            table[1].value = mval[1];
            table[3].value = mval[2];
            table[5].value = mval[1];
            table[7].value = mval[3];
            table[3].bits = 3;
            table[7].bits = 3;
            table_size = 8;
        }
        _ => unreachable!(),
    }

    // Replicate to fill ROOT_BITS entries.
    while table_size != goal_size {
        for index in 0..table_size {
            table[(table_size + index) as usize] = table[index as usize];
        }
        table_size <<= 1;
    }
    table_size
}

fn replicate_value(table: &mut [HuffmanCode], offset: u32, step: i32, mut end: i32, code: HuffmanCode) {
    loop {
        end -= step;
        table[offset as usize + end as usize] = code;
        if end <= 0 {
            break;
        }
    }
}

fn next_table_bit_size(count: &[u16], mut len: i32, root_bits: i32) -> i32 {
    let mut left: i32 = 1 << (len - root_bits);
    while len < MAX_CODE_LENGTH as i32 {
        left -= count[len as usize] as i32;
        if left <= 0 {
            break;
        }
        len += 1;
        left <<= 1;
    }
    len - root_bits
}

fn reverse_bits(num: u32) -> u32 {
    REVERSE_BITS[num as usize] as u32
}

/// Decode a Huffman symbol from a lookup table + bit reader.
///
/// Peeks the next 8 bits, looks up in the root table, and if the
/// entry points to a 2nd-level sub-table, follows the pointer.
///
/// Returns the decoded symbol value.
pub fn decode_symbol(table: &[HuffmanCode], peek_bits: u32) -> (u16, u8) {
    let root_entry = table[(peek_bits & 0xFF) as usize];
    if root_entry.bits as u32 > ROOT_BITS {
        // 2nd-level lookup.
        let nbits = root_entry.bits as u32 - ROOT_BITS;
        let sub_index = root_entry.value as u32 + ((peek_bits >> ROOT_BITS) & bit_mask(nbits));
        let sub_entry = table[sub_index as usize];
        (sub_entry.value, sub_entry.bits + ROOT_BITS as u8)
    } else {
        (root_entry.value, root_entry.bits)
    }
}

fn bit_mask(n: u32) -> u32 {
    (1u32 << n) - 1
}
