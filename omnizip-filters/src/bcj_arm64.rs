//! BCJ ARM64 (AArch64) filter — converts ARM64 BL and ADRP relative
//! addresses to absolute values for better compression.
//!
//! - **BL instruction** (bits 31-26 = `100101`): 26-bit immediate,
//!   range ±128 MiB.
//! - **ADRP instruction** (bits 31-29 vary, bits 28-24 = `10000`):
//!   ±512 MiB range; the filter only converts in-range values.
//!
//! Ported from `xz-utils/src/liblzma/simple/arm64.c` (0BSD license).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::Filter;

/// ARM64 / AArch64 (little-endian) BCJ filter.
pub struct BcjArm64Filter;

impl Filter for BcjArm64Filter {
    fn name(&self) -> &'static str {
        "bcj-arm64"
    }

    fn encode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        arm64_transform(&mut output, true);
        output
    }

    fn decode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        arm64_transform(&mut output, false);
        output
    }
}

fn arm64_transform(data: &mut [u8], is_encoder: bool) {
    let len = data.len() & !3usize;
    let mut i = 0usize;
    while i + 4 <= len {
        let instr = read_le_32(&data[i..i + 4]);
        if instr >> 26 == 0x25 {
            // BL: convert full 26-bit immediate.
            let src = instr;
            let pc = (i as u32) >> 2;
            let pc = if is_encoder {
                pc
            } else {
                0u32.wrapping_sub(pc)
            };
            let new_instr = 0x9400_0000u32 | ((src.wrapping_add(pc)) & 0x03FF_FFFF);
            write_le_32(&mut data[i..i + 4], new_instr);
        } else if (instr & 0x9F00_0000) == 0x9000_0000 {
            // ADRP: extract src from scattered bit fields.
            let src = ((instr >> 29) & 3) | ((instr >> 3) & 0x001F_FFFC);
            // Range check: only convert if the target is within ±512 MiB.
            if (src.wrapping_add(0x0002_0000)) & 0x001C_0000 != 0 {
                i += 4;
                continue;
            }
            let pc = (i as u32) >> 12;
            let pc = if is_encoder {
                pc
            } else {
                0u32.wrapping_sub(pc)
            };
            let dest = src.wrapping_add(pc);
            let mut new_instr = instr & 0x9000_001F;
            new_instr |= (dest & 3) << 29;
            new_instr |= (dest & 0x0003_FFFC) << 3;
            new_instr |= (0u32.wrapping_sub(dest & 0x0002_0000)) & 0x00E0_0000;
            write_le_32(&mut data[i..i + 4], new_instr);
        }
        i += 4;
    }
}

fn read_le_32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn write_le_32(bytes: &mut [u8], val: u32) {
    let le = val.to_le_bytes();
    bytes[..4].copy_from_slice(&le);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_no_branches() {
        let data = [0u8; 32];
        let enc = BcjArm64Filter.encode(&data);
        let dec = BcjArm64Filter.decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trips_with_bl() {
        let mut data = vec![0u8; 64];
        // BL instruction: bits 31-26 = 100101 → top 6 bits = 0x25.
        // 0x25 << 26 = 0x94000000.
        let instr: u32 = 0x9400_0000 | 0x0010_0000; // BL with nonzero offset
        write_le_32(&mut data[..4], instr);
        write_le_32(&mut data[4..8], instr);
        let enc = BcjArm64Filter.encode(&data);
        let dec = BcjArm64Filter.decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn short_input_no_panic() {
        let _ = BcjArm64Filter.encode(&[0u8; 2]);
        let _ = BcjArm64Filter.decode(&[0u8; 2]);
    }
}
