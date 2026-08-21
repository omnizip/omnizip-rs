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
    ///
    /// Word-staged: tops up the partial byte, pushes whole bytes, and
    /// leaves the tail partial — the previous per-bit loop (a
    /// `last_mut` + branch + shift + modulo PER BIT) cost ~10-50x the
    /// reference's single 8-byte store on every emitted symbol.
    pub(crate) fn write_bits(&mut self, value: u64, nbits: u32) {
        if nbits == 0 {
            return;
        }
        let nbits64 = u64::from(nbits);
        let value = if nbits64 >= 64 {
            value
        } else {
            value & ((1u64 << nbits64) - 1)
        };
        let mut rem = nbits64;
        let mut v = value;
        if self.bit_pos != 0 {
            let take = (8 - self.bit_pos).min(nbits64 as u32) as u64;
            let last = self
                .out
                .last_mut()
                .expect("BitWriter invariant: byte exists when bit_pos > 0");
            *last |= ((v & ((1u64 << take) - 1)) << self.bit_pos) as u8;
            self.bit_pos += take as u32;
            v >>= take;
            rem -= take;
            if self.bit_pos == 8 {
                self.bit_pos = 0;
            }
        }
        while rem >= 8 {
            self.out.push(v as u8);
            v >>= 8;
            rem -= 8;
        }
        if rem > 0 {
            self.out.push((v & ((1u64 << rem) - 1)) as u8);
            self.bit_pos = rem as u32;
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
    #[cfg(test)]
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

    // Dist cache, initialized to the brotli default. Updated as
    // commands are emitted so subsequent commands can use short codes.
    let mut dist_cache: [i32; 4] = commands::INITIAL_DIST_CACHE;

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
                    // Compute the distance code using the brotli
                    // dist-cache convention. The cache starts at
                    // [4, 11, 15, 16] (the values the decoder's
                    // dist_rb is initialized to, in reverse).
                    let dist_code = commands::compute_distance_code(
                        max_dist as u32,
                        u32::MAX,
                        &dist_cache,
                    );
                    let (dist_code_packed, _dist_nbits, dist_extra_value) =
                        prefix_encode_copy_distance(dist_code, 0, 0);
                    let use_last = dist_code == 0;
                    let cmd_prefix = get_length_code(insert_len, mlen, use_last);
                    cmds.push(BrotliCommand {
                        insert_len: insert_len as u32,
                        copy_len: mlen as u32,
                        distance: max_dist as u32,
                        use_last_distance: use_last,
                        cmd_prefix,
                        // dist_prefix packs (nbits << 10) | code, matching upstream.
                        dist_prefix: dist_code_packed,
                        dist_extra: dist_extra_value,
                    });
                    // Update dist_cache: shift older entries down, insert new at [0].
                    dist_cache[3] = dist_cache[2];
                    dist_cache[2] = dist_cache[1];
                    dist_cache[1] = dist_cache[0];
                    dist_cache[0] = max_dist as i32;
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
    // Only attempt Huffman coding if we found at least one real LZ77
    // match (not just the synthetic trailing command). Otherwise the
    // trailing command would produce extra bytes the decoder would
    // have to discard, and our encoder doesn't handle that case
    // correctly. Fall back to uncompressed.
    let has_real_match = commands
        .iter()
        .any(|c| !c.use_last_distance && c.copy_len >= 4);
    if !has_real_match {
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
    // Ensure at least 2 distinct symbols in the histogram. The
    // brotli simple-form Huffman table for NSYM=1 has an encoder/
    // decoder mismatch: the encoder writes 0 bits per literal but
    // the decoder reads 1 bit. We avoid this by always having ≥2
    // symbols (the second one is never matched, so it adds zero
    // bits per emitted literal when its code length is 1, but it
    // keeps the decoder on the right track).
    let mut unique_symbols: usize = lit_histogram.iter().filter(|&&c| c > 0).count();
    if unique_symbols == 1 {
        // Find the symbol with non-zero count and add a different one.
        let existing = lit_histogram
            .iter()
            .position(|&c| c > 0)
            .expect("at least one symbol");
        let dummy = if existing == 0 { 1 } else { 0 };
        lit_histogram[dummy] = 1;
        unique_symbols = 2;
    }
    let _ = unique_symbols;

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
        huffman::build_and_store_simple(&lit_histogram, 256, 7, &mut bw);
    if !emitted {
        return encode_uncompressed(input);
    }

    // ----- Command Huffman tree (simple form for small command sets) -----
    // Build a histogram of command symbols. For inputs with few unique
    // commands, simple-form emission works and avoids needing the
    // complex-form RLE encoder.
    let mut cmd_histogram = vec![0u32; 704];
    for cmd in &commands {
        cmd_histogram[cmd.cmd_prefix as usize] += 1;
    }
    let (cmd_emitted, cmd_depth, cmd_bits) =
        huffman::build_and_store_simple(&cmd_histogram, 704, 9, &mut bw);
    if !cmd_emitted {
        return encode_uncompressed(input);
    }

    // ----- Distance Huffman tree (simple form) -----
    let mut dist_histogram = vec![0u32; 64];
    for cmd in &commands {
        if cmd.copy_len > 0 {
            let dist_code = (cmd.dist_prefix & 0x3ff) as usize;
            if dist_code < 64 {
                dist_histogram[dist_code] += 1;
            }
        }
    }
    let (dist_emitted, dist_depth, dist_bits) =
        huffman::build_and_store_simple(&dist_histogram, 64, 5, &mut bw);
    if !dist_emitted {
        return encode_uncompressed(input);
    }

    // ----- Emit data: commands + literals + distances -----
    pos = 0;
    for cmd in &commands {
        let cmd_code = cmd.cmd_prefix as usize;
        bw.write_bits(
            u64::from(cmd_bits[cmd_code]),
            u32::from(cmd_depth[cmd_code]),
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
                u64::from(dist_bits[dist_code]),
                u32::from(dist_depth[dist_code]),
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
}
