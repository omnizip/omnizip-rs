//! BCJ PowerPC filter — converts PowerPC (big-endian) branch targets
//! for better compression.
//!
//! PowerPC unconditional/conditional branch instruction format:
//!   primary opcode 6 bits | offset 24 bits | Abs(1) | Link(1)
//!   Primary opcode 0x12 (18) = branch.
//!
//! Ported from `xz-utils/src/liblzma/simple/powerpc.c` (0BSD license).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::Filter;

/// PowerPC (32-bit, big-endian) BCJ filter.
pub struct BcjPowerPcFilter;

impl Filter for BcjPowerPcFilter {
    fn name(&self) -> &'static str {
        "bcj-powerpc"
    }

    fn encode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        ppc_transform(&mut output, true);
        output
    }

    fn decode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        ppc_transform(&mut output, false);
        output
    }
}

fn ppc_transform(data: &mut [u8], is_encoder: bool) {
    let len = data.len() & !3usize;
    let mut i = 0usize;
    while i + 4 <= len {
        // Check primary opcode (high 6 bits = 0x12 = 18) and
        // the low 2 bits of byte 3 (Abs=1 indicates absolute branch).
        if (data[i] >> 2) == 0x12 && (data[i + 3] & 3) == 1 {
            let src = ((u32::from(data[i]) & 3) << 24)
                | (u32::from(data[i + 1]) << 16)
                | (u32::from(data[i + 2]) << 8)
                | (u32::from(data[i + 3]) & !3u32);
            let pos = i as u32;
            let dest = if is_encoder {
                src.wrapping_add(pos)
            } else {
                src.wrapping_sub(pos)
            };
            data[i] = 0x48 | ((dest >> 24) as u8 & 0x03);
            data[i + 1] = (dest >> 16) as u8;
            data[i + 2] = (dest >> 8) as u8;
            data[i + 3] &= 0x03;
            data[i + 3] |= dest as u8;
        }
        i += 4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_no_branches() {
        let data = [0u8; 32];
        let enc = BcjPowerPcFilter.encode(&data);
        let dec = BcjPowerPcFilter.decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trips_with_branch() {
        let mut data = vec![0u8; 64];
        // Primary opcode 0x12 in high 6 bits → byte = 0x48 (0x12 << 2).
        // Abs bit = 1 (low bit of byte 3).
        data[0] = 0x48;
        data[3] = 0x01;
        data[32] = 0x48;
        data[35] = 0x01;
        let enc = BcjPowerPcFilter.encode(&data);
        let dec = BcjPowerPcFilter.decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn short_input_no_panic() {
        let _ = BcjPowerPcFilter.encode(&[0u8; 2]);
    }
}
