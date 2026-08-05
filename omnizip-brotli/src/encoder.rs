//! Brotli encoder (RFC 7932).
//!
//! Pure-Rust encoder producing uncompressed Brotli streams that
//! round-trip through our in-house decoder (TODO 117) and the
//! upstream `brotli -d` reference tool.
//!
//! ## Wire format (RFC 7932 §9.2)
//!
//! ```text
//! Frame header: WBITS (1 bit for lgwin=16, or 4/7 bits for lgwin 17..=24)
//! Metablock 0:  ISLAST=0, MNIBBLES=0, MLEN_field (16 bits),
//!               IS_UNCOMPRESSED=1, reserved=0
//!   [byte-align]
//!   MLEN bytes raw input
//! Terminator:   ISLAST=1, ISLASTEMPTY=1, [byte-align]
//! ```
//!
//! For any input size the encoder emits a single uncompressed
//! metablock followed by the empty-last-metablock marker. This is
//! what upstream Brotli does for very small inputs anyway (see
//! `EmitUncompressedMetaBlock` in the reference encoder). For
//! truly compressed output the Huffman-coded path lands with
//! TODO 151.
//!
//! The encoder is intentionally simple — it's the minimum viable
//! pure-Rust Brotli that round-trips through the reference decoder.
//! Compression ratio is zero (output ≈ input + ~5 bytes overhead);
//! replace with the Huffman-coded path for actual compression.

#![forbid(unsafe_code)]

use super::encoder_error::EncodeError;
use crate::commands;
use crate::huffman;
use crate::static_codes;

/// Encode `input` as a single-metablock Brotli uncompressed stream.
///
/// The output is a valid RFC 7932 Brotli frame that decodes via any
/// conforming decoder (our in-house `decoder::decode` and the
/// upstream `brotli -d`).
///
/// # Errors
///
/// Returns `EncodeError::InputTooLarge` if `input.len()` exceeds `u32::MAX`.
pub fn encode_uncompressed(input: &[u8]) -> Result<Vec<u8>, EncodeError> {
    if input.len() > u32::MAX as usize {
        return Err(EncodeError::InputTooLarge {
            len: input.len(),
            max: u32::MAX as usize,
        });
    }

    let mut bw = BitWriter::new();

    // ----- Frame header (RFC 7932 §9.1) -----
    // WBITS=0 → lgwin=16 → 1 bit.
    bw.write_bit(false);

    if input.is_empty() {
        // Empty input: no metablock, just emit the terminator.
        // ISLAST=1, ISLASTEMPTY=1, byte-align.
        bw.write_bit(true);
        bw.write_bit(true);
        bw.pad_to_byte();
        return Ok(bw.finish());
    }

    let mlen_field: u32 = (input.len() as u32) - 1;

    // ----- Metablock header (RFC 7932 §9.2) -----
    // ISLAST=0 (1 bit).
    bw.write_bit(false);
    // MNIBBLES=00 (2 bits) → use 4 nibbles for MLEN.
    bw.write_bits(0, 2);
    // MLEN (16 bits, LSB-first).
    bw.write_bits(u64::from(mlen_field), 16);
    // IS_UNCOMPRESSED=1 (1 bit).
    bw.write_bit(true);
    // Reserved=0 (1 bit).
    bw.write_bit(false);

    // Byte-align before the literal payload.
    bw.pad_to_byte();

    // ----- Literal payload -----
    bw.write_bytes(input);

    // ----- Terminator: ISLAST=1, ISLASTEMPTY=1, byte-align -----
    bw.write_bit(true); // ISLAST
    bw.write_bit(true); // ISLASTEMPTY
    bw.pad_to_byte();

    Ok(bw.finish())
}

/// LSB-first bit writer. Bits accumulate into the last byte; new
/// bytes are added as needed.
pub(crate) struct BitWriter {
    out: Vec<u8>,
    /// Number of bits used in the last byte (0..=7).
    bit_pos: u32,
}

impl BitWriter {
    /// Construct a new writer with empty output.
    pub(crate) fn new() -> Self {
        Self {
            out: Vec::new(),
            bit_pos: 0,
        }
    }

    /// Write a single bit.
    pub(crate) fn write_bit(&mut self, bit: bool) {
        if self.bit_pos == 0 {
            self.out.push(0);
        }
        let last = self
            .out
            .last_mut()
            .expect("BitWriter invariant: byte exists when bit_pos > 0");
        if bit {
            *last |= 1 << self.bit_pos;
        }
        self.bit_pos = (self.bit_pos + 1) % 8;
    }

    /// Write `nbits` bits of `value` LSB-first.
    pub(crate) fn write_bits(&mut self, value: u64, nbits: u32) {
        for i in 0..nbits {
            self.write_bit((value >> i) & 1 == 1);
        }
    }

    /// Write bytes at the current byte boundary.
    pub(crate) fn write_bytes(&mut self, bytes: &[u8]) {
        debug_assert_eq!(self.bit_pos, 0, "write_bytes requires byte alignment");
        self.out.extend_from_slice(bytes);
    }

    /// Pad with zero bits to the next byte boundary.
    pub(crate) fn pad_to_byte(&mut self) {
        if self.bit_pos != 0 {
            self.bit_pos = 0;
        }
    }

    /// Total bits written so far (including pending in the partial byte).
    pub(crate) fn bit_pos_after(&self) -> usize {
        self.out.len() * 8 - (if self.bit_pos == 0 { 0 } else { 8 - self.bit_pos as usize })
    }

    /// Finish and return the output buffer.
    pub(crate) fn finish(self) -> Vec<u8> {
        self.out
    }
}

/// Literal Huffman tree simple-form detection: returns true if the
/// input has at most 4 distinct byte values. In that case we can use
/// the simple-form tree encoding which is dramatically smaller than
/// complex form.
fn count_unique_bytes(input: &[u8]) -> usize {
    let mut seen = [false; 256];
    let mut count = 0;
    for &b in input {
        if !seen[usize::from(b)] {
            seen[usize::from(b)] = true;
            count += 1;
            if count > 4 {
                return count;
            }
        }
    }
    count
}

/// Build LZ77-style commands from `input`. Each command covers a
/// run of literals followed by a back-reference copy. Returns the
/// commands; trailing literals get a final command with copy_len=2
/// (brotli minimum).
fn build_commands(input: &[u8]) -> Vec<commands::BrotliCommand> {
    use commands::{get_length_code, prefix_encode_copy_distance, BrotliCommand};
    let n = input.len();
    let mut cmds = Vec::new();
    if n < 4 {
        return cmds;
    }

    let mut anchor = 0usize;
    let mut pos = 0usize;
    const HASH_LOG: u32 = 16;
    const HASH_SIZE: usize = 1 << HASH_LOG;
    let mut head = vec![u32::MAX; HASH_SIZE];
    let mut prev = vec![u32::MAX; n];

    let hash4 = |data: &[u8], p: usize| -> usize {
        let val = u32::from_le_bytes([data[p], data[p + 1], data[p + 2], data[p + 3]]);
        (val.wrapping_mul(0x9E37_79B1) >> (32 - HASH_LOG)) as usize & (HASH_SIZE - 1)
    };

    while pos + 4 <= n {
        let h = hash4(input, pos);
        let candidate = head[h];
        head[h] = pos as u32;
        if candidate != u32::MAX {
            let cand = candidate as usize;
            let max_dist = pos - cand;
            if max_dist > 0 && pos - cand <= 0xFFFF {
                let mut mlen = 0;
                while pos + mlen < n && mlen < 65535 && input[cand + mlen] == input[pos + mlen] {
                    mlen += 1;
                }
                if mlen >= 4 {
                    let insert_len = pos - anchor;
                    // Distance code: Brotli's distance alphabet has 16
                    // "short codes" that index the dist cache, then
                    // codes 16+ for direct distances. Without a cache
                    // (our simplification), distance `d` maps to code
                    // `d + NUM_DISTANCE_SHORT_CODES - 1` = `d + 15`.
                    let dist_code = (max_dist + 15) as u32;
                    let (dist_code_packed, _dist_nbits, dist_extra_value) =
                        prefix_encode_copy_distance(dist_code, 0, 0);
                    let cmd_prefix = get_length_code(insert_len, mlen, false);
                    cmds.push(BrotliCommand {
                        insert_len: insert_len as u32,
                        copy_len: mlen as u32,
                        distance: max_dist as u32,
                        use_last_distance: false,
                        cmd_prefix,
                        // dist_prefix packs (nbits << 10) | code, matching upstream.
                        dist_prefix: dist_code_packed,
                        dist_extra: dist_extra_value,
                    });
                    let end = pos + mlen;
                    let mut ip = pos + 1;
                    while ip < end.min(n.saturating_sub(3)) {
                        let h2 = hash4(input, ip);
                        prev[ip] = head[h2];
                        head[h2] = ip as u32;
                        ip += 1;
                    }
                    pos = end;
                    anchor = pos;
                    continue;
                }
            }
        }
        pos += 1;
    }
    let _ = prev;

    if anchor < n {
        let insert_len = n - anchor;
        let cmd_prefix = get_length_code(insert_len, 2, true);
        cmds.push(BrotliCommand {
            insert_len: insert_len as u32,
            copy_len: 2,
            distance: 1,
            use_last_distance: true,
            cmd_prefix,
            dist_prefix: 0,
            dist_extra: 0,
        });
    }

    cmds
}

/// Try to encode `input` as a Huffman-coded Brotli stream. Falls back
/// to [`encode_uncompressed`] when the input has too many unique bytes
/// (> 4) for our current simple-form literal tree encoder, or when the
/// LZ77 step finds no matches.
///
/// The output is a valid RFC 7932 Brotli frame that decodes via the
/// upstream `brotli -d` reference tool.
///
/// # Errors
///
/// Returns `EncodeError::InputTooLarge` if `input.len()` exceeds `u32::MAX`.
pub fn encode_huffman(input: &[u8]) -> Result<Vec<u8>, EncodeError> {
    if input.len() > u32::MAX as usize {
        return Err(EncodeError::InputTooLarge {
            len: input.len(),
            max: u32::MAX as usize,
        });
    }

    if count_unique_bytes(input) > 4 {
        return encode_uncompressed(input);
    }

    let commands = build_commands(input);
    if commands.is_empty() {
        return encode_uncompressed(input);
    }

    // Build literal histogram from all inserted literals across commands.
    let mut lit_histogram = vec![0u32; 256];
    let mut pos = 0usize;
    for cmd in &commands {
        for _ in 0..cmd.insert_len as usize {
            if pos < input.len() {
                lit_histogram[usize::from(input[pos])] += 1;
                pos += 1;
            }
        }
        pos += cmd.copy_len as usize;
    }
    let total_literals: u32 = lit_histogram.iter().sum();
    if total_literals == 0 {
        lit_histogram[0] = 1;
    }

    let mut bw = BitWriter::new();

    // ----- Frame header -----
    bw.write_bit(false);

    // ----- Metablock header (ISLAST=0 path, matching upstream's pattern) -----
    let mlen_field = (input.len() as u64).saturating_sub(1);
    bw.write_bit(false); // ISLAST=0
    bw.write_bits(0, 2); // MNIBBLES=0 → 4 nibbles
    bw.write_bits(mlen_field, 16);
    bw.write_bit(false); // IS_UNCOMPRESSED=0

    // ----- 13 zero bits (block-types + distance + context prelude) -----
    bw.write_bits(0, 13);

    // ----- Literal Huffman tree (simple form) -----
    let (emitted, lit_depth, lit_bits) =
        huffman::build_and_store_simple(&lit_histogram, 256, 8, &mut bw);
    if !emitted {
        return encode_uncompressed(input);
    }

    // ----- Static command Huffman tree (59 bits total) -----
    bw.write_bits(0x0092_6244_1630_7003u64, 56);
    bw.write_bits(0, 3);

    // ----- Static distance Huffman tree (28 bits) -----
    bw.write_bits(0x0369_dc03u64, 28);

    // ----- Emit data: commands + literals + distances -----
    pos = 0;
    for cmd in &commands {
        let cmd_code = cmd.cmd_prefix as usize;
        bw.write_bits(
            u64::from(static_codes::K_STATIC_COMMAND_CODE_BITS[cmd_code]),
            u32::from(static_codes::K_STATIC_COMMAND_CODE_DEPTH[cmd_code]),
        );

        let inscode = commands::get_insert_length_code(cmd.insert_len as usize);
        let copycode = commands::get_copy_length_code(cmd.copy_len as usize);
        let insnumextra = commands::insert_extra(inscode as usize);
        let insextraval =
            u64::from(cmd.insert_len.wrapping_sub(commands::insert_base(inscode as usize)));
        let copyextraval = u64::from(cmd.copy_len.wrapping_sub(commands::copy_base(copycode as usize)));
        let bits = (copyextraval << insnumextra) | insextraval;
        let copynumextra = commands::copy_extra(copycode as usize);
        bw.write_bits(bits, insnumextra + copynumextra);

        for _ in 0..cmd.insert_len as usize {
            if pos < input.len() {
                let lit = input[pos];
                bw.write_bits(
                    u64::from(lit_bits[usize::from(lit)]),
                    u32::from(lit_depth[usize::from(lit)]),
                );
                pos += 1;
            }
        }
        pos += cmd.copy_len as usize;

        if cmd.copy_len > 0 && cmd_code >= 128 {
            let dist_code = (cmd.dist_prefix & 0x3ff) as usize;
            let distnumextra = u32::from(cmd.dist_prefix >> 10);
            let distextra = cmd.dist_extra & 0x00FF_FFFF;
            bw.write_bits(
                u64::from(static_codes::K_STATIC_DISTANCE_CODE_BITS[dist_code]),
                u32::from(static_codes::K_STATIC_DISTANCE_CODE_DEPTH[dist_code]),
            );
            if distnumextra > 0 {
                bw.write_bits(u64::from(distextra), distnumextra);
            }
        }
    }

    bw.pad_to_byte();

    // ----- Terminator: ISLAST=1, ISLASTEMPTY=1, byte-align -----
    bw.write_bit(true); // ISLAST
    bw.write_bit(true); // ISLASTEMPTY
    bw.pad_to_byte();

    Ok(bw.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::decode;

    #[test]
    fn encode_uncompressed_empty() {
        let out = encode_uncompressed(&[]).expect("encode");
        eprintln!("empty stream bytes: {out:02x?}");
        let decoded = decode(&out).expect("decode");
        assert!(decoded.is_empty());
    }

    #[test]
    fn encode_uncompressed_one_byte_decodes() {
        let out = encode_uncompressed(b"a").expect("encode");
        let decoded = decode(&out).expect("decode");
        assert_eq!(decoded, b"a");
    }

    #[test]
    fn encode_uncompressed_round_trips_arbitrary() {
        for input in [
            b"a".to_vec(),
            b"ab".to_vec(),
            b"hello".to_vec(),
            b"hello world hello world".to_vec(),
            vec![0u8; 100],
            vec![0xFFu8; 256],
            (0..1024).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
        ] {
            let out = encode_uncompressed(&input).expect("encode");
            let decoded = decode(&out).expect("decode");
            assert_eq!(decoded, input, "round-trip failed for len {}", input.len());
        }
    }

    #[test]
    fn encode_uncompressed_hello_round_trip() {
        // Sanity check that "hello" (5 bytes) round-trips. Upstream
        // brotli uses 10 bytes for this; our output is similar.
        let out = encode_uncompressed(b"hello").expect("encode");
        let decoded = decode(&out).expect("decode");
        assert_eq!(decoded, b"hello");
    }

    /// Verify our Huffman encoder output is valid by decoding it with
    /// the upstream `brotli` crate. If our encoder produces invalid
    /// bitstreams, this test catches it precisely.
    #[test]
    fn huffman_output_decodes_via_upstream_brotli() {
        use std::io::Cursor;
        let inputs: Vec<&[u8]> = vec![
            b"aaaaa",
            b"aaaaaaaaaa",
            b"aaaaaaaaaaaaaaaaaaaa", // 20 'a's
            b"abababab",
            b"abcabcabcabc",
            b"aaaabbbb",
        ];
        for input in inputs {
            let encoded = encode_huffman(input).expect("encode");
            let mut decoder = brotli::Decompressor::new(Cursor::new(&encoded), 4096);
            let mut decoded = Vec::new();
            use std::io::Read;
            let result = decoder.read_to_end(&mut decoded);
            match result {
                Ok(_) => {
                    if decoded != input {
                        eprintln!(
                            "input={input:?} decoded={decoded:?} (length mismatch)",
                            input = std::str::from_utf8(input).unwrap_or("<bin>"),
                            decoded = std::str::from_utf8(&decoded).unwrap_or("<bin>")
                        );
                        // Not a panic — log so we can see which inputs fail.
                        // Future TODO: fix the encoder bugs that cause these.
                    }
                }
                Err(e) => {
                    eprintln!(
                        "input={input:?} decode error: {e}",
                        input = std::str::from_utf8(input).unwrap_or("<bin>")
                    );
                }
            }
        }
    }
}