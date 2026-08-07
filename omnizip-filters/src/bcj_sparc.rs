//! BCJ SPARC filter — converts SPARC v9 CALL instructions for better
//! compression.
//!
//! SPARC CALL instruction: opcode `01` (bits 31-30) | 30-bit displacement.
//! The filter handles two byte-0 patterns that correspond to CALL: 0x40
//! and 0x7F (after the bit-twiddling convention used by the reference).
//!
//! Ported from `xz-utils/src/liblzma/simple/sparc.c` (0BSD license).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::Filter;

/// SPARC (32-bit, big-endian) BCJ filter.
pub struct BcjSparcFilter;

impl Filter for BcjSparcFilter {
    fn name(&self) -> &'static str {
        "bcj-sparc"
    }

    fn encode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        sparc_transform(&mut output, true);
        output
    }

    fn decode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        sparc_transform(&mut output, false);
        output
    }
}

fn sparc_transform(data: &mut [u8], is_encoder: bool) {
    let len = data.len() & !3usize;
    let mut i = 0usize;
    while i + 4 <= len {
        let b0 = data[i];
        let b1 = data[i + 1];
        let is_call = (b0 == 0x40 && (b1 & 0xC0) == 0x00) || (b0 == 0x7F && (b1 & 0xC0) == 0xC0);
        if is_call {
            let src = (u32::from(b0) << 24)
                | (u32::from(b1) << 16)
                | (u32::from(data[i + 2]) << 8)
                | u32::from(data[i + 3]);
            let src_shifted = src << 2;
            let pos = i as u32;
            let dest = if is_encoder {
                src_shifted.wrapping_add(pos)
            } else {
                src_shifted.wrapping_sub(pos)
            };
            let dest_shifted = dest >> 2;
            // Normalise the sign-bit and OR in the CALL opcode marker.
            let normalised = (((0u32.wrapping_sub((dest_shifted >> 22) & 1)) << 22) & 0x3FFF_FFFF)
                | (dest_shifted & 0x003F_FFFF)
                | 0x4000_0000;
            data[i] = (normalised >> 24) as u8;
            data[i + 1] = (normalised >> 16) as u8;
            data[i + 2] = (normalised >> 8) as u8;
            data[i + 3] = normalised as u8;
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
        let enc = BcjSparcFilter.encode(&data);
        let dec = BcjSparcFilter.decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trips_with_call() {
        let mut data = vec![0u8; 64];
        // CALL: byte 0 = 0x40, byte1 high 2 bits = 00.
        data[0] = 0x40;
        data[32] = 0x40;
        let enc = BcjSparcFilter.encode(&data);
        let dec = BcjSparcFilter.decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn short_input_no_panic() {
        let _ = BcjSparcFilter.encode(&[0u8; 2]);
    }
}
