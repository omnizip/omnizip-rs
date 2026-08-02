//! BCJ ARM (32-bit) filter — converts ARM BL (Branch with Link) relative
//! addresses to a form that compresses better.
//!
//! ARM BL instruction: condition(4) | 1011(4) | offset(24)
//! The 24-bit offset is shifted left 2 and added to PC+8. The filter
//! normalizes the target to its absolute address.
//!
//! Ported from `xz-utils/src/liblzma/simple/arm.c` (0BSD license).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::Filter;

/// ARM (32-bit, little-endian) BCJ filter.
pub struct BcjArmFilter;

impl Filter for BcjArmFilter {
    fn name(&self) -> &'static str {
        "bcj-arm"
    }

    fn encode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        arm_transform(&mut output, true);
        output
    }

    fn decode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        arm_transform(&mut output, false);
        output
    }
}

/// ARM BL opcode high byte (little-endian: stored at byte 3 of the
/// 4-byte instruction).
const BL_HIGH_BYTE: u8 = 0xEB;

fn arm_transform(data: &mut [u8], is_encoder: bool) {
    let len = data.len() & !3usize; // round down to multiple of 4
    let mut i = 0usize;
    while i + 4 <= len {
        if data[i + 3] == BL_HIGH_BYTE {
            let src = (u32::from(data[i + 2]) << 16)
                | (u32::from(data[i + 1]) << 8)
                | u32::from(data[i]);
            let src_shifted = src << 2;
            let pos = i as u32;
            let dest = if is_encoder {
                src_shifted.wrapping_add(pos.wrapping_add(8))
            } else {
                src_shifted.wrapping_sub(pos.wrapping_add(8))
            };
            let dest_shifted = dest >> 2;
            data[i + 2] = (dest_shifted >> 16) as u8;
            data[i + 1] = (dest_shifted >> 8) as u8;
            data[i] = dest_shifted as u8;
        }
        i += 4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_no_branches() {
        let data = [0x00u8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        let enc = BcjArmFilter.encode(&data);
        assert_eq!(enc, data);
        let dec = BcjArmFilter.decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trips_with_bl() {
        // BL instruction: byte3=0xEB, bytes 0-2 = offset.
        // Without conversion, the 3-byte offset changes per BL site.
        let mut data = vec![0u8; 64];
        data[3] = 0xEB;
        data[0] = 0x10;
        data[1] = 0x20;
        data[2] = 0x30;
        data[35] = 0xEB;
        data[32] = 0x40;
        data[33] = 0x50;
        data[34] = 0x60;
        let enc = BcjArmFilter.encode(&data);
        let dec = BcjArmFilter.decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn encode_changes_offset_bytes() {
        let mut data = vec![0u8; 8];
        data[3] = 0xEB;
        data[0] = 0x01;
        let enc = BcjArmFilter.encode(&data);
        // Encoder must modify at least one byte after the BL.
        assert_ne!(enc, data);
    }

    #[test]
    fn short_input_no_panic() {
        let data = [0u8; 3];
        let _ = BcjArmFilter.encode(&data);
        let _ = BcjArmFilter.decode(&data);
    }
}
