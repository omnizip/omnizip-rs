#![allow(non_upper_case_globals)]
// Represents the range of values belonging to a prefix code:
// [offset, offset + 2^nbits)
pub struct PrefixCodeRange {
  pub offset: u16,
  pub nbits: u8,
}

pub const kBlockLengthPrefixCode: [PrefixCodeRange; 26] = [PrefixCodeRange {
                                                             offset: 1,
                                                             nbits: 2,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 5,
                                                             nbits: 2,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 9,
                                                             nbits: 2,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 13,
                                                             nbits: 2,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 17,
                                                             nbits: 3,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 25,
                                                             nbits: 3,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 33,
                                                             nbits: 3,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 41,
                                                             nbits: 3,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 49,
                                                             nbits: 4,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 65,
                                                             nbits: 4,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 81,
                                                             nbits: 4,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 97,
                                                             nbits: 4,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 113,
                                                             nbits: 5,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 145,
                                                             nbits: 5,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 177,
                                                             nbits: 5,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 209,
                                                             nbits: 5,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 241,
                                                             nbits: 6,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 305,
                                                             nbits: 6,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 369,
                                                             nbits: 7,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 497,
                                                             nbits: 8,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 753,
                                                             nbits: 9,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 1265,
                                                             nbits: 10,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 2289,
                                                             nbits: 11,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 4337,
                                                             nbits: 12,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 8433,
                                                             nbits: 13,
                                                           },
                                                           PrefixCodeRange {
                                                             offset: 16625,
                                                             nbits: 24,
                                                           }];


#[derive(Debug, Copy, Clone)]
pub struct CmdLutElement {
  pub insert_len_extra_bits: u8,
  pub copy_len_extra_bits: u8,
  pub distance_code: i8,
  pub context: u8,
  pub insert_len_offset: u16,
  pub copy_len_offset: u16,
}
pub const kCmdLut: [CmdLutElement; 704] = build_cmd_lut();

const fn build_cmd_lut() -> [CmdLutElement; 704] {
    const K_INSERT_LENGTH_EXTRA_BITS: [u8; 24] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x02, 0x02, 0x03, 0x03,
        0x04, 0x04, 0x05, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0C, 0x0E, 0x18,
    ];
    const K_COPY_LENGTH_EXTRA_BITS: [u8; 24] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x02, 0x02,
        0x03, 0x03, 0x04, 0x04, 0x05, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x18,
    ];
    const K_CELL_POS: [usize; 11] = [0, 1, 0, 1, 8, 9, 2, 16, 10, 17, 18];

    let mut insert_length_offsets: [u16; 24] = [0; 24];
    let mut copy_length_offsets: [u16; 24] = [0; 24];
    insert_length_offsets[0] = 0;
    copy_length_offsets[0] = 2;
    let mut i: usize = 0;
    while i < 23 {
        insert_length_offsets[i + 1] =
            insert_length_offsets[i] + (1u16 << K_INSERT_LENGTH_EXTRA_BITS[i]);
        copy_length_offsets[i + 1] =
            copy_length_offsets[i] + (1u16 << K_COPY_LENGTH_EXTRA_BITS[i]);
        i += 1;
    }

    let zero = CmdLutElement {
        insert_len_extra_bits: 0,
        copy_len_extra_bits: 0,
        distance_code: 0,
        context: 0,
        insert_len_offset: 0,
        copy_len_offset: 0,
    };
    let mut lut: [CmdLutElement; 704] = [zero; 704];

    let mut symbol: usize = 0;
    while symbol < 704 {
        let cell_idx = symbol >> 6;
        let cell_pos = K_CELL_POS[cell_idx];
        let copy_code = ((cell_pos << 3) & 0x18) + (symbol & 0x7);
        let insert_code = (cell_pos & 0x18) + ((symbol >> 3) & 0x7);
        let copy_len_offset = copy_length_offsets[copy_code];
        let insert_len_offset = insert_length_offsets[insert_code];
        let context: u8 = if copy_len_offset > 4 { 3 } else { (copy_len_offset - 2) as u8 };
        let distance_code: i8 = if cell_idx >= 2 { -1 } else { 0 };
        lut[symbol] = CmdLutElement {
            insert_len_extra_bits: K_INSERT_LENGTH_EXTRA_BITS[insert_code],
            copy_len_extra_bits: K_COPY_LENGTH_EXTRA_BITS[copy_code],
            distance_code,
            context,
            insert_len_offset,
            copy_len_offset,
        };
        symbol += 1;
    }
    lut
}
