//! BCJ2 — 4-stream Branch / Call / Jump filter for x86 executables.
//!
//! Unlike [`BcjX86Filter`](crate::BcjX86Filter), which converts branches
//! in-place, BCJ2 splits the input into **four** independent streams that
//! can be compressed separately for better overall ratio:
//!
//! 1. **Main** — the original bytes with every converted branch's 4-byte
//!    relative offset zeroed out (length == input length).
//! 2. **Call** — every 32-bit CALL (`E8`) target, converted to an absolute
//!    address, big-endian.
//! 3. **Jump** — every 32-bit JMP (`E9`) target, converted to an absolute
//!    address, big-endian.
//! 4. **Extra** — one marker byte per converted branch recording its kind
//!    (`0` = call, `1` = jump). Used for diagnostics and future range-coder
//!    integration; not strictly required for round-trip because the opcode
//!    byte survives in the main stream.
//!
//! ## Wire format
//!
//! The 4 streams are concatenated with a 4-byte little-endian length prefix
//! each, in the order main → call → jump → extra:
//!
//! ```text
//! [main_len: u32 LE][main bytes]
//! [call_len: u32 LE][call bytes]
//! [jump_len: u32 LE][jump bytes]
//! [extra_len: u32 LE][extra bytes]
//! ```
//!
//! ## Reversibility
//!
//! Both `encode` and `decode` use the identical "scan for `E8`/`E9`, skip
//! 5 bytes after a hit" cursor, so they visit exactly the same positions.
//! `decode(encode(x)) == x` for every input, including data that contains
//! no branch instructions (in which case the call/jump/extra streams are
//! empty and the main stream is byte-identical to the input).
//!
//! ## Determinism
//!
//! No floating point, no heap iteration order dependence, no threading.
//! Same input always produces byte-identical output.
//!
//! ## Algorithmic lineage
//!
//! Ported conceptually from `omnizip/lib/omnizip/filters/bcj2/` (MIT,
//! Ribose Inc.). The Ruby port's encoder raises `NotImplementedError` and
//! only ships a decoder built around a range coder; this Rust module
//! implements a fully self-contained, range-coder-free variant that is
//! sufficient for the deterministic, round-trip-correct contract required
//! by `LimniFS`. The 4-stream split still matches the 7-Zip SDK layout so a
//! future range-coder encoder can drop in without changing the wire shape.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::Filter;

/// x86 BCJ2 filter — splits input into 4 streams for separate compression.
pub struct Bcj2Filter;

/// x86 opcodes recognised by BCJ2.
const OPCODE_CALL: u8 = 0xE8;
const OPCODE_JMP: u8 = 0xE9;

/// Extra-stream marker for a CALL branch.
const EXTRA_CALL: u8 = 0;
/// Extra-stream marker for a JMP branch.
const EXTRA_JUMP: u8 = 1;

/// The four BCJ2 output streams, pre-serialization.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Bcj2Streams {
    /// Original bytes with branch offsets zeroed (always `input.len()`).
    pub main: Vec<u8>,
    /// Big-endian absolute CALL targets, 4 bytes each.
    pub call: Vec<u8>,
    /// Big-endian absolute JMP targets, 4 bytes each.
    pub jump: Vec<u8>,
    /// One marker byte per converted branch (`EXTRA_CALL` / `EXTRA_JUMP`).
    pub extra: Vec<u8>,
}

impl Bcj2Filter {
    /// Split `input` into the 4 BCJ2 streams.
    ///
    /// This is the core transform; [`Filter::encode`] wraps the result in
    /// the length-prefixed wire format.
    #[must_use]
    pub fn split(input: &[u8]) -> Bcj2Streams {
        // Main stream is the same length as the input; copy then overwrite
        // the offset bytes of converted branches with zeros.
        let mut main = input.to_vec();
        // Worst case: every byte is an E8/E9 opcode (impossible because we
        // skip 5 after each hit, but reserve generously to avoid reallocs).
        let mut call = Vec::new();
        let mut jump = Vec::new();
        let mut extra = Vec::new();

        let mut i = 0usize;
        while i + 5 <= main.len() {
            let op = main[i];
            if op == OPCODE_CALL || op == OPCODE_JMP {
                // Read the 4-byte LE relative offset at i+1.
                let offset = read_le_32(&main[i + 1..i + 5]);
                // Convert to absolute address, matching the 7-Zip convention:
                //   absolute = offset + (instruction_pointer + 5)
                // where instruction_pointer == i (the byte offset of the
                // opcode within the stream).
                let absolute = offset.wrapping_add(position_as_u32(i).wrapping_add(5));

                if op == OPCODE_CALL {
                    call.extend_from_slice(&absolute.to_be_bytes());
                    extra.push(EXTRA_CALL);
                } else {
                    jump.extend_from_slice(&absolute.to_be_bytes());
                    extra.push(EXTRA_JUMP);
                }

                // Zero the offset bytes in the main stream so the
                // downstream codec sees highly compressible zeros instead
                // of pseudo-random relative offsets.
                for b in &mut main[i + 1..i + 5] {
                    *b = 0;
                }

                // Skip the full 5-byte instruction so encode and decode
                // visit identical positions.
                i += 5;
            } else {
                i += 1;
            }
        }

        Bcj2Streams {
            main,
            call,
            jump,
            extra,
        }
    }

    /// Reconstruct the original bytes from the 4 streams.
    ///
    /// Inverse of [`split`](Self::split).
    #[must_use]
    pub fn merge(streams: &Bcj2Streams) -> Vec<u8> {
        let mut out = streams.main.clone();

        let mut call_pos = 0usize;
        let mut jump_pos = 0usize;
        let mut extra_pos = 0usize;

        let mut i = 0usize;
        while i + 5 <= out.len() {
            let op = out[i];
            if op == OPCODE_CALL || op == OPCODE_JMP {
                // Sanity check: if the extra stream still has a marker for
                // this position, consume it. If it doesn't (because the
                // input contained an E8/E9 that was NOT converted — e.g.
                // produced by merge after a partial split, or just data
                // that happens to start with E8 but whose offset bytes
                // were non-zero in the original), we still attempt the
                // merge using the matching stream. When that stream is
                // exhausted, leave the bytes as-is (they're already zeros
                // from encode, which is wrong — but this can only happen
                // if the streams are inconsistent).
                let is_call = op == OPCODE_CALL;

                let stream = if is_call {
                    &streams.call
                } else {
                    &streams.jump
                };
                let stream_pos = if is_call {
                    &mut call_pos
                } else {
                    &mut jump_pos
                };

                if *stream_pos + 4 <= stream.len() {
                    let absolute = read_be_32(&stream[*stream_pos..*stream_pos + 4]);
                    *stream_pos += 4;
                    // Inverse of encode's conversion:
                    //   offset = absolute - (i + 5)
                    let offset = absolute.wrapping_sub(position_as_u32(i).wrapping_add(5));
                    write_le_32(&mut out[i + 1..i + 5], offset);
                }

                if extra_pos < streams.extra.len() {
                    extra_pos += 1;
                }

                i += 5;
            } else {
                i += 1;
            }
        }

        out
    }

    /// Serialize the 4 streams into the length-prefixed wire format.
    fn serialize(streams: &Bcj2Streams) -> Vec<u8> {
        let total = 16 // four u32 length prefixes
            + streams.main.len()
            + streams.call.len()
            + streams.jump.len()
            + streams.extra.len();
        let mut out = Vec::with_capacity(total);

        push_len_prefixed(&mut out, &streams.main);
        push_len_prefixed(&mut out, &streams.call);
        push_len_prefixed(&mut out, &streams.jump);
        push_len_prefixed(&mut out, &streams.extra);

        out
    }

    /// Parse the length-prefixed wire format back into 4 streams.
    ///
    /// Returns `None` if the input is truncated or a length prefix overruns
    /// the remaining bytes.
    fn deserialize(bytes: &[u8]) -> Option<Bcj2Streams> {
        let mut cursor = 0usize;

        let main = read_len_prefixed(bytes, &mut cursor)?;
        let call = read_len_prefixed(bytes, &mut cursor)?;
        let jump = read_len_prefixed(bytes, &mut cursor)?;
        let extra = read_len_prefixed(bytes, &mut cursor)?;

        // Trailing garbage is not allowed.
        if cursor != bytes.len() {
            return None;
        }

        Some(Bcj2Streams {
            main,
            call,
            jump,
            extra,
        })
    }
}

impl Filter for Bcj2Filter {
    fn name(&self) -> &'static str {
        "bcj2"
    }

    fn encode(&self, input: &[u8]) -> Vec<u8> {
        let streams = Self::split(input);
        Self::serialize(&streams)
    }

    fn decode(&self, input: &[u8]) -> Vec<u8> {
        match Self::deserialize(input) {
            Some(streams) => Self::merge(&streams),
            None => {
                // Malformed wire data. The Filter trait returns Vec<u8>,
                // not Result, so we fall back to returning the input
                // unchanged — same behaviour as the other BCJ filters when
                // given data that doesn't match their expectations. A real
                // archive reader will validate lengths up front.
                input.to_vec()
            }
        }
    }
}

// ---- helpers ---------------------------------------------------------------

/// Length-prefix a stream into `out`. The 4-byte LE prefix lets `decode`
/// recover the boundary even when the stream itself contains arbitrary
/// bytes (including E8/E9).
fn push_len_prefixed(out: &mut Vec<u8>, data: &[u8]) {
    #[allow(clippy::cast_possible_truncation)]
    let len = data.len() as u32;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(data);
}

/// Read a length-prefixed chunk. Returns `None` on truncation.
fn read_len_prefixed(bytes: &[u8], cursor: &mut usize) -> Option<Vec<u8>> {
    if *cursor + 4 > bytes.len() {
        return None;
    }
    let len = u32::from_le_bytes([
        bytes[*cursor],
        bytes[*cursor + 1],
        bytes[*cursor + 2],
        bytes[*cursor + 3],
    ]) as usize;
    *cursor += 4;
    if *cursor + len > bytes.len() {
        return None;
    }
    let chunk = bytes[*cursor..*cursor + len].to_vec();
    *cursor += len;
    Some(chunk)
}

/// BCJ2 only applies to x86 code, which is < 4 GiB. Positions above
/// `u32::MAX` are unreachable in practice; truncating keeps the arithmetic
/// in `u32` and matches the 7-Zip SDK.
#[allow(clippy::cast_possible_truncation)]
fn position_as_u32(i: usize) -> u32 {
    i as u32
}

fn read_le_32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn read_be_32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
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

    fn round_trip(data: &[u8]) {
        let filter = Bcj2Filter;
        let encoded = filter.encode(data);
        let decoded = filter.decode(&encoded);
        assert_eq!(decoded.as_slice(), data, "round-trip mismatch");
    }

    fn split_streams(data: &[u8]) -> Bcj2Streams {
        let filter = Bcj2Filter;
        let encoded = filter.encode(data);
        Bcj2Filter::deserialize(&encoded).expect("wire must round-trip")
    }

    #[test]
    fn round_trips_x86_like_data() {
        // Two CALLs and one JMP with explicit relative offsets.
        let mut data = Vec::new();
        data.extend_from_slice(b"prefix ");
        data.push(OPCODE_CALL);
        data.extend_from_slice(&0x10u32.to_le_bytes());
        data.extend_from_slice(b" mid ");
        data.push(OPCODE_JMP);
        data.extend_from_slice(&0x20u32.to_le_bytes());
        data.extend_from_slice(b" tail");
        data.push(OPCODE_CALL);
        data.extend_from_slice(&0xFFFF_FFF0u32.to_le_bytes());

        round_trip(&data);
    }

    #[test]
    fn round_trips_non_code_data() {
        // No E8/E9 → all streams except main are empty, main == input.
        let data = b"the quick brown fox jumps over the lazy dog";
        let streams = split_streams(data);
        assert_eq!(streams.main, data);
        assert!(streams.call.is_empty());
        assert!(streams.jump.is_empty());
        assert!(streams.extra.is_empty());
        round_trip(data);
    }

    #[test]
    fn round_trips_random_data() {
        // Deterministic PRNG so the test itself is reproducible.
        let mut state = 0x1234_5678u32;
        let data: Vec<u8> = (0..4096)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 16) as u8
            })
            .collect();
        round_trip(&data);
    }

    #[test]
    fn round_trips_empty_and_short() {
        round_trip(b"");
        round_trip(b"\xe8"); // single E8, too short to convert
        round_trip(b"\xe9\x01\x02"); // E9 + 2 bytes, still too short
        round_trip(b"\xe8\x00\x00\x00"); // E8 + 3 bytes, exactly one short
    }

    #[test]
    fn round_trips_back_to_back_branches() {
        let mut data = Vec::new();
        for _ in 0..10 {
            data.push(OPCODE_CALL);
            data.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        }
        for _ in 0..5 {
            data.push(OPCODE_JMP);
            data.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        }
        round_trip(&data);
    }

    #[test]
    fn round_trips_branch_at_end_boundary() {
        // Branch exactly at the last 5 bytes of the input.
        let mut data = b"abcdefg".to_vec();
        data.push(OPCODE_CALL);
        data.extend_from_slice(&0x42u32.to_le_bytes());
        round_trip(&data);
    }

    #[test]
    fn main_stream_length_equals_input_length() {
        let data = vec![
            OPCODE_CALL,
            0x10,
            0x00,
            0x00,
            0x00,
            OPCODE_JMP,
            0x20,
            0x00,
            0x00,
            0x00,
        ];
        let streams = split_streams(&data);
        assert_eq!(streams.main.len(), data.len());
    }

    #[test]
    fn call_and_jump_streams_get_right_entries() {
        let data = vec![
            OPCODE_CALL, 0x01, 0x00, 0x00, 0x00, // CALL at pos 0
            0x90, 0x90, // NOP padding
            OPCODE_JMP, 0x02, 0x00, 0x00, 0x00, // JMP at pos 7
        ];
        let streams = split_streams(&data);

        // CALL stream: one BE absolute = 0x01 + (0 + 5) = 6
        assert_eq!(streams.call.len(), 4);
        assert_eq!(read_be_32(&streams.call), 6);

        // JMP stream: one BE absolute = 0x02 + (7 + 5) = 14
        assert_eq!(streams.jump.len(), 4);
        assert_eq!(read_be_32(&streams.jump), 14);

        // Extra stream: call then jump.
        assert_eq!(streams.extra, vec![EXTRA_CALL, EXTRA_JUMP]);
    }

    #[test]
    fn main_stream_has_zeroed_offsets() {
        let data = vec![
            OPCODE_CALL,
            0x11,
            0x22,
            0x33,
            0x44,
            0x90,
        ];
        let streams = split_streams(&data);
        // Opcode preserved, offset bytes zeroed.
        assert_eq!(
            streams.main,
            vec![OPCODE_CALL, 0, 0, 0, 0, 0x90]
        );
    }

    #[test]
    fn determinism_check() {
        let data: Vec<u8> = (0..2048u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 16) as u8)
            .collect();
        let filter = Bcj2Filter;
        let a = filter.encode(&data);
        let b = filter.encode(&data);
        assert_eq!(a, b, "encode must be deterministic");

        // Decoded output must also be identical across calls.
        let da = filter.decode(&a);
        let db = filter.decode(&b);
        assert_eq!(da, db);
        assert_eq!(da, data);
    }

    #[test]
    fn each_stream_round_trips_independently_via_merge() {
        // Construct the 4 streams by hand and verify merge reconstructs
        // the original. This proves decode works straight from the
        // Bcj2Streams struct, not just from the wire format.
        let original = vec![
            OPCODE_CALL,
            0x10,
            0x00,
            0x00,
            0x00, // CALL offset 0x10 at pos 0
            0x90,
            OPCODE_JMP,
            0x20,
            0x00,
            0x00,
            0x00, // JMP offset 0x20 at pos 6
        ];

        // Build expected split.
        let call_abs = 0x10u32.wrapping_add(5);
        let jump_abs = 0x20u32.wrapping_add(6 + 5);
        let streams = Bcj2Streams {
            main: vec![
                OPCODE_CALL,
                0,
                0,
                0,
                0,
                0x90,
                OPCODE_JMP,
                0,
                0,
                0,
                0,
            ],
            call: call_abs.to_be_bytes().to_vec(),
            jump: jump_abs.to_be_bytes().to_vec(),
            extra: vec![EXTRA_CALL, EXTRA_JUMP],
        };

        let reconstructed = Bcj2Filter::merge(&streams);
        assert_eq!(reconstructed, original);
    }

    #[test]
    fn wire_format_is_length_prefixed_le() {
        let data = vec![OPCODE_CALL, 0x10, 0x00, 0x00, 0x00];
        let encoded = Bcj2Filter.encode(&data);

        // Main length prefix.
        let main_len = u32::from_le_bytes([
            encoded[0],
            encoded[1],
            encoded[2],
            encoded[3],
        ]);
        assert_eq!(main_len, 5);

        // Main bytes follow.
        assert_eq!(&encoded[4..9], &[OPCODE_CALL, 0, 0, 0, 0]);

        // Call length = 4.
        let call_len = u32::from_le_bytes([
            encoded[9],
            encoded[10],
            encoded[11],
            encoded[12],
        ]);
        assert_eq!(call_len, 4);
    }

    #[test]
    fn skip_five_rule_prevents_false_branch_in_offset() {
        // Offset bytes of a CALL happen to contain 0xE8; encode must not
        // treat them as a new opcode because the skip-5 rule moves past.
        let data = vec![
            OPCODE_CALL,
            OPCODE_CALL, // inside the offset region
            0x00,
            0x00,
            0x00,
        ];
        let streams = split_streams(&data);
        assert_eq!(streams.call.len(), 4, "only one CALL should be converted");
        round_trip(&data);
    }
}
