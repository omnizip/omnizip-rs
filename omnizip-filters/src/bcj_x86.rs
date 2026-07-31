//! BCJ x86 filter — Branch / Call / Jump conversion for x86 / `x86_64`.
//!
//! x86 branch instructions (CALL `E8`, JMP `E9`) store their target as
//! a 32-bit relative offset from the next instruction. These offsets are
//! effectively random across binaries, so they compress poorly.
//!
//! The BCJ-x86 filter scans for `E8`/`E9` opcodes and converts the
//! following 4-byte relative offset into a value with better locality.
//! On decode, the same positions are visited and the inverse conversion
//! recovers the original bytes.
//!
//! ## Reversibility guarantee
//!
//! Both `encode` and `decode` use the same byte-by-byte scan with the
//! same "skip 5 after a branch" rule. This ensures they visit identical
//! positions, making the round-trip exact for every input.
//!
//! Ported from `omnizip/lib/omnizip/filters/bcj_x86.rb` (MIT, Ribose
//! Inc.), simplified to the reversible core algorithm.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::Filter;

/// x86 / `x86_64` BCJ filter. Encodes by converting relative branch offsets
/// to pseudo-absolute; decodes by reversing the conversion.
pub struct BcjX86Filter;

impl Filter for BcjX86Filter {
    fn name(&self) -> &'static str {
        "bcj-x86"
    }

    fn encode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        forward_transform(&mut output);
        output
    }

    fn decode(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        inverse_transform(&mut output);
        output
    }
}

const CALL_OPCODE: u8 = 0xE8;
const JMP_OPCODE: u8 = 0xE9;

/// Forward transform: for each `E8`/`E9` at position `i`, add `(i + 5)`
/// to the 4-byte LE offset at `i+1`. Skip 5 bytes after each branch to
/// guarantee encode and decode visit the same positions.
fn forward_transform(data: &mut [u8]) {
    let mut i = 0usize;
    while i + 5 <= data.len() {
        let op = data[i];
        if op == CALL_OPCODE || op == JMP_OPCODE {
            let offset = read_le_32(&data[i + 1..i + 5]);
            let pos = position_as_u32(i);
            let absolute = offset.wrapping_add(pos.wrapping_add(5));
            write_le_32(&mut data[i + 1..i + 5], absolute);
            i += 5;
        } else {
            i += 1;
        }
    }
}

/// Inverse transform: subtract `(i + 5)` from the 4-byte value at
/// `i+1` for each `E8`/`E9`. Must visit the same positions as
/// [`forward_transform`].
fn inverse_transform(data: &mut [u8]) {
    let mut i = 0usize;
    while i + 5 <= data.len() {
        let op = data[i];
        if op == CALL_OPCODE || op == JMP_OPCODE {
            let absolute = read_le_32(&data[i + 1..i + 5]);
            let pos = position_as_u32(i);
            let offset = absolute.wrapping_sub(pos.wrapping_add(5));
            write_le_32(&mut data[i + 1..i + 5], offset);
            i += 5;
        } else {
            i += 1;
        }
    }
}

/// Convert a byte position to u32. BCJ-x86 only applies to executable
/// code, which is always < 4 GiB; positions above `u32::MAX` are
/// unreachable in practice.
#[allow(clippy::cast_possible_truncation)]
fn position_as_u32(i: usize) -> u32 {
    i as u32
}

fn read_le_32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn write_le_32(bytes: &mut [u8], value: u32) {
    let [b0, b1, b2, b3] = value.to_le_bytes();
    bytes[0] = b0;
    bytes[1] = b1;
    bytes[2] = b2;
    bytes[3] = b3;
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_pure_branches() {
        let filter = BcjX86Filter;
        let data = vec![
            CALL_OPCODE,
            0x10,
            0x00,
            0x00,
            0x00,
            JMP_OPCODE,
            0x20,
            0x00,
            0x00,
            0x00,
        ];
        let encoded = filter.encode(&data);
        assert_ne!(encoded, data, "encode should modify the offsets");
        let decoded = filter.decode(&encoded);
        assert_eq!(decoded, data, "decode must recover the original");
    }

    #[test]
    fn round_trips_mixed_content() {
        let filter = BcjX86Filter;
        let mut data = b"prefix ".to_vec();
        data.extend_from_slice(&[CALL_OPCODE, 0x10, 0x00, 0x00, 0x00]);
        data.extend_from_slice(b" middle ");
        data.extend_from_slice(&[JMP_OPCODE, 0x20, 0x00, 0x00, 0x00]);
        data.extend_from_slice(b" suffix");
        let encoded = filter.encode(&data);
        let decoded = filter.decode(&encoded);
        assert_eq!(decoded, data);
    }

    #[test]
    fn leaves_pure_text_unchanged() {
        let filter = BcjX86Filter;
        let data = b"the quick brown fox jumps over the lazy dog";
        let encoded = filter.encode(data);
        assert_eq!(encoded.as_slice(), data);
    }

    #[test]
    fn handles_empty_and_short_input() {
        let filter = BcjX86Filter;
        assert!(filter.encode(b"").is_empty());
        assert!(filter.decode(b"").is_empty());
        let short = b"\xe8\x00\x00";
        assert_eq!(filter.encode(short).as_slice(), short);
    }

    #[test]
    fn handles_back_to_back_branches() {
        let filter = BcjX86Filter;
        let data = vec![
            CALL_OPCODE,
            0xFF,
            0xFF,
            0xFF,
            0xFF,
            CALL_OPCODE,
            0x01,
            0x00,
            0x00,
            0x00,
            JMP_OPCODE,
            0x80,
            0x00,
            0x00,
            0x00,
        ];
        let encoded = filter.encode(&data);
        let decoded = filter.decode(&encoded);
        assert_eq!(decoded, data);
    }

    #[test]
    fn skips_e8_in_data_region() {
        let filter = BcjX86Filter;
        let data = vec![CALL_OPCODE, CALL_OPCODE, 0x00, 0x00, 0x00, 0x90];
        let encoded = filter.encode(&data);
        let decoded = filter.decode(&encoded);
        assert_eq!(decoded, data);
    }
}
