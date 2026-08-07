//! BCJ IA-64 (Itanium) filter — converts IA-64 branch/call
//! instructions for better compression.
//!
//! IA-64 instructions are grouped into 128-bit bundles, each holding
//! three 41-bit instructions and a 5-bit template field. The template
//! at byte 0 (low 5 bits) determines which slots contain branch-type
//! instructions (per `BRANCH_TABLE`).
//!
//! For each branch slot, the filter extracts the 20+1-bit target
//! immediate from scattered bit fields, converts it to an absolute
//! address, and writes it back.
//!
//! Ported from `xz-utils/src/liblzma/simple/ia64.c` (0BSD license).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::Filter;

/// IA-64 Itanium (little-endian) BCJ filter.
pub struct BcjIa64Filter;

impl Filter for BcjIa64Filter {
    fn name(&self) -> &'static str {
        "bcj-ia64"
    }

    fn encode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        ia64_transform(&mut output, true);
        output
    }

    fn decode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        ia64_transform(&mut output, false);
        output
    }
}

/// 5-bit template → bitmask of which 3 slots contain branches.
/// Copied verbatim from the C reference `BRANCH_TABLE[32]`.
const BRANCH_TABLE: [u8; 32] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 6, 6, 0, 0, 7, 7, 4, 4, 0, 0, 4, 4, 0, 0,
];

fn ia64_transform(data: &mut [u8], is_encoder: bool) {
    let len = data.len() & !15usize; // round down to multiple of 16
    let mut i = 0usize;
    while i + 16 <= len {
        let instr_template = u32::from(data[i] & 0x1F);
        let mask = BRANCH_TABLE[instr_template as usize];
        let mut bit_pos = 5u32;

        for slot in 0..3u32 {
            if bit_pos >= 128 {
                break;
            }
            let active = (mask >> slot) & 1 != 0;
            if active {
                process_slot(data, i, bit_pos, is_encoder);
            }
            bit_pos += 41;
        }
        i += 16;
    }
}

fn process_slot(data: &mut [u8], bundle_start: usize, bit_pos: u32, is_encoder: bool) {
    let byte_pos = (bit_pos >> 3) as usize;
    let bit_res = bit_pos & 7;

    // Read 6 bytes (48 bits) starting at bundle_start + byte_pos.
    let mut instruction: u64 = 0;
    for j in 0..6usize {
        instruction |= u64::from(data[bundle_start + j + byte_pos]) << (8 * j);
    }

    let inst_norm = instruction >> bit_res;

    // Check for branch-type instruction:
    //   bits 37-40 = 0x5 (form 5), bits 9-11 = 0.
    if (inst_norm >> 37) & 0xF == 0x5 && (inst_norm >> 9) & 0x7 == 0 {
        let mut src = ((inst_norm >> 13) & 0xFFFFF) as u32;
        src |= (((inst_norm >> 36) & 1) as u32) << 20;
        let src_shifted = src << 4;

        let pos = bundle_start as u32;
        let dest = if is_encoder {
            src_shifted.wrapping_add(pos)
        } else {
            src_shifted.wrapping_sub(pos)
        };
        let dest_shifted = dest >> 4;

        // Clear the 21 target bits and write the new value.
        const TARGET_MASK: u64 = 0x1F_FFFF; // 21 bits
        let mut new_inst_norm = inst_norm & !(TARGET_MASK << 13);
        new_inst_norm |= u64::from(dest_shifted & 0xFFFFF) << 13;
        new_inst_norm |= u64::from(dest_shifted & 0x100000) << (36 - 20);

        instruction &= if bit_res == 0 {
            0
        } else {
            (1u64 << bit_res) - 1
        };
        instruction |= new_inst_norm << bit_res;

        for j in 0..6usize {
            data[bundle_start + j + byte_pos] = (instruction >> (8 * j)) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_no_branches() {
        let data = [0u8; 32];
        let enc = BcjIa64Filter.encode(&data);
        let dec = BcjIa64Filter.decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trips_data_with_template() {
        // Template 16 (0b10000) has branches in slots 0 and 1.
        // Set byte0 low 5 bits = 16 (0x10).
        let mut data = vec![0u8; 64];
        data[0] = 0x10;
        data[16] = 0x10;
        let enc = BcjIa64Filter.encode(&data);
        let dec = BcjIa64Filter.decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn short_input_no_panic() {
        let _ = BcjIa64Filter.encode(&[0u8; 8]);
    }
}
