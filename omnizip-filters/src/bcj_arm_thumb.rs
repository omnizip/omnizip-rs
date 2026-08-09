//! BCJ ARM-Thumb filter — converts ARM Thumb BL (Branch with Link)
//! relative addresses to a form that compresses better.
//!
//! Thumb BL is a 32-bit instruction split across two 16-bit half-words:
//!   first half:  11110 | `offset_high(10)`
//!   second half: 11x11 | `offset_low(11)`   (x = link bit, usually 1)
//!
//! The filter extracts the 22-bit combined offset, shifts left 1,
//! normalizes to absolute, and writes back.
//!
//! Ported from `xz-utils/src/liblzma/simple/armthumb.c` (0BSD license).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::Filter;

/// ARM-Thumb (Thumb-1, little-endian) BCJ filter.
pub struct BcjArmThumbFilter;

impl Filter for BcjArmThumbFilter {
    fn name(&self) -> &'static str {
        "bcj-arm-thumb"
    }

    fn encode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        arm_thumb_transform(&mut output, true);
        output
    }

    fn decode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        arm_thumb_transform(&mut output, false);
        output
    }
}

fn arm_thumb_transform(data: &mut [u8], is_encoder: bool) {
    if data.len() < 4 {
        return;
    }
    let limit = data.len() - 4;
    let mut i = 0usize;
    while i <= limit {
        if (data[i + 1] & 0xF8) == 0xF0 && (data[i + 3] & 0xF8) == 0xF8 {
            let src = ((u32::from(data[i + 1]) & 7) << 19)
                | (u32::from(data[i]) << 11)
                | ((u32::from(data[i + 3]) & 7) << 8)
                | u32::from(data[i + 2]);
            let src_shifted = src << 1;
            let pos = i as u32;
            let dest = if is_encoder {
                src_shifted.wrapping_add(pos.wrapping_add(4))
            } else {
                src_shifted.wrapping_sub(pos.wrapping_add(4))
            };
            let dest_shifted = dest >> 1;
            data[i + 1] = 0xF0 | ((dest_shifted >> 19) as u8 & 0x7);
            data[i] = (dest_shifted >> 11) as u8;
            data[i + 3] = 0xF8 | ((dest_shifted >> 8) as u8 & 0x7);
            data[i + 2] = dest_shifted as u8;
            i += 4;
        } else {
            i += 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_no_branches() {
        let data = [0u8; 32];
        let enc = BcjArmThumbFilter.encode(&data);
        assert_eq!(enc, data);
        let dec = BcjArmThumbFilter.decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trips_with_bl() {
        let mut data = vec![0u8; 64];
        // BL first half: byte1 high 5 bits = 11110 (0xF0-0xF7).
        data[1] = 0xF0;
        data[0] = 0x12;
        // BL second half: byte3 high 5 bits = 11111 or 11101 (0xF8-0xFF or 0xE8-0xEF).
        data[3] = 0xF8;
        data[2] = 0x34;
        let enc = BcjArmThumbFilter.encode(&data);
        let dec = BcjArmThumbFilter.decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn short_input_no_panic() {
        let _ = BcjArmThumbFilter.encode(&[0u8; 2]);
        let _ = BcjArmThumbFilter.decode(&[0u8; 2]);
    }
}
