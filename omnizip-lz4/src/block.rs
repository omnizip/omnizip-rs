//! LZ4 block-format encoder + decoder (pure-Rust, from spec).
//!
//! Implements the LZ4 block format described at
//! <https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md>.
//! The encoder is a port of the C reference's fast loop, so its output
//! matches `lz4 -1` structurally (see [`compress_block`]).
//!
//! ## Block format
//!
//! ```text
//! token (1 byte): (literal_length_code << 4) | match_length_code
//! [literal_length_extension]  — if code == 15, read 0xFF bytes until < 255
//! literal_bytes
//! match_offset (2 bytes LE)   — 0 = end-of-block marker
//! [match_length_extension]    — if code == 15, same as literal extension
//! ```
//!
//! The last sequence is a literal-only token (no match offset).

#![forbid(unsafe_code)]

/// Minimum match length (LZ4 spec).
const MIN_MATCH: usize = 4;
/// Last 5 bytes of a block are always literals (`LASTLITERALS`).
const LAST_LITERALS: usize = 5;
/// A match must start at least 12 bytes before the end (`MFLIMIT`).
const MF_LIMIT: usize = 12;
/// Inputs below this length are emitted as literals only (`LZ4_minLength`).
const LZ4_MIN_LENGTH: usize = MF_LIMIT + 1;
/// Skip-stride shift: after 64 consecutive misses the fast loop starts
/// skipping positions (`LZ4_skipTrigger`).
const SKIP_TRIGGER: u32 = 6;
/// `acceleration = 1` — the reference CLI's `-1` setting.
const ACCELERATION: usize = 1;
/// Maximum representable match offset.
const DISTANCE_MAX: usize = 65535;
/// Hash table log2 size — upstream's `LZ4_MEMORY_USAGE` (14) − 2.
const HASH_LOG: u32 = 12;
const HASH_SIZE: usize = 1 << HASH_LOG;
/// `prime5bytes` from lz4.c's `LZ4_hash5`.
const PRIME_5_BYTES: u64 = 889_523_592_379;
/// `LZ4_MAX_INPUT_SIZE` — beyond this the C library refuses to compress
/// (u32 hash indices would be ambiguous); we degrade to literals.
const MAX_INPUT_SIZE: usize = 0x7E00_0000;

/// Compress `input` into an LZ4 block (no size prefix, no frame wrapper).
///
/// Line-by-line port of `LZ4_compress_generic` (lz4.c, `byU32` mode,
/// `noDict`, `notLimited`, `acceleration = 1`) — the algorithm behind
/// `lz4 -1`. Matching that reference exactly keeps ratio parity; the
/// little-endian hash variant is pinned so output is identical across
/// machines (`LimniFS` determinism).
///
/// Structure: a find-loop hashes each visited position with a 5-byte key
/// into a 4096-entry table, growing its stride after every 64 misses
/// (reset once a match is emitted — the fast exit on incompressible data);
/// found matches are extended backwards into the literal run ("catch up"),
/// then the byte right after a match is re-tested so consecutive matches
/// can be emitted with zero literals between them.
// The length and narrow casts mirror lz4.c's single-function structure;
// every cast is bounded (positions < `MAX_INPUT_SIZE`, nibbles < 16).
#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
#[must_use]
pub fn compress_block(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        return vec![0u8];
    }
    let iend = input.len();
    let mut out = Vec::with_capacity(iend + iend / 255 + 16);
    let mut anchor = 0usize;

    if !(LZ4_MIN_LENGTH..=MAX_INPUT_SIZE).contains(&iend) {
        write_token_literals(&mut out, iend);
        out.extend_from_slice(input);
        return out;
    }

    let mflimit_plus_one = iend - (MF_LIMIT - 1);
    let matchlimit = iend - LAST_LITERALS;
    let mut table = [0u32; HASH_SIZE];
    // Upstream inserts position 0 under its hash before advancing; a
    // zero-initialized table holds 0 there anyway, so only the advance
    // and the first forward hash remain.
    let mut ip = 1usize;
    let mut forward_h = hash5(input, 1);

    'main: loop {
        // Find a match: single hash probe per visited position; stride
        // grows every SKIP_TRIGGER consecutive misses, then resets for
        // the next round once this one succeeds.
        let mut m_pos;
        {
            let mut forward_ip = ip;
            let mut step = 1usize;
            let mut search_match_nb = ACCELERATION << SKIP_TRIGGER;
            let found;
            loop {
                let h = forward_h;
                let current = forward_ip;
                let match_index = table[h] as usize;
                ip = forward_ip;
                forward_ip += step;
                step = search_match_nb >> SKIP_TRIGGER;
                search_match_nb += 1;

                if forward_ip > mflimit_plus_one {
                    break 'main;
                }
                forward_h = hash5(input, forward_ip);
                table[h] = current as u32;

                if match_index + DISTANCE_MAX < current {
                    continue;
                }
                if input[match_index..match_index + MIN_MATCH] == input[ip..ip + MIN_MATCH] {
                    found = match_index;
                    break;
                }
            }
            m_pos = found;
        }

        // Catch up: extend the match backwards through the literal run.
        if m_pos > 0 && input[ip - 1] == input[m_pos - 1] {
            loop {
                ip -= 1;
                m_pos -= 1;
                if !(ip > anchor && m_pos > 0 && input[ip - 1] == input[m_pos - 1]) {
                    break;
                }
            }
        }

        // Encode literals into a token, kept for the match-length nibble.
        let lit_len = ip - anchor;
        let mut token_pos = out.len();
        out.push(0);
        if lit_len >= 15 {
            out[token_pos] = 15 << 4;
            write_length_ext(&mut out, lit_len - 15);
        } else {
            out[token_pos] = (lit_len as u8) << 4;
        }
        out.extend_from_slice(&input[anchor..ip]);

        // Encode match(es). After each one the very next position is
        // re-tested; a hit there emits another match with zero literals
        // (`token = 0`, upstream's `_next_match` retry).
        loop {
            let offset = ip - m_pos;
            out.extend_from_slice(&(offset as u16).to_le_bytes());
            let match_code = count_equal(input, ip + MIN_MATCH, m_pos + MIN_MATCH, matchlimit);
            ip += match_code + MIN_MATCH;
            if match_code >= 15 {
                out[token_pos] |= 15;
                write_length_ext(&mut out, match_code - 15);
            } else {
                out[token_pos] |= match_code as u8;
            }
            anchor = ip;

            if ip >= mflimit_plus_one {
                break 'main;
            }

            // Fill table for ip-2 (a position skipped over by the match).
            let h = hash5(input, ip - 2);
            table[h] = (ip - 2) as u32;

            // Test next position.
            let h = hash5(input, ip);
            let current = ip;
            let match_index = table[h] as usize;
            table[h] = current as u32;
            if match_index + DISTANCE_MAX >= current
                && input[match_index..match_index + MIN_MATCH] == input[ip..ip + MIN_MATCH]
            {
                m_pos = match_index;
                token_pos = out.len();
                out.push(0);
                continue;
            }

            // Prepare next loop.
            ip += 1;
            forward_h = hash5(input, ip);
            continue 'main;
        }
    }

    write_token_literals(&mut out, iend - anchor);
    out.extend_from_slice(&input[anchor..]);
    out
}

/// Decompress an LZ4 block. `expected_len` is cross-checked.
///
/// # Errors
///
/// Returns `&'static str` on malformed input.
pub fn decompress_block(compressed: &[u8], expected_len: usize) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::with_capacity(expected_len);
    let mut i = 0usize;

    while i < compressed.len() {
        let token = compressed[i];
        i += 1;

        // Literal length.
        let mut lit_len = usize::from(token >> 4);
        if lit_len == 15 {
            loop {
                if i >= compressed.len() {
                    return Err("literal length extends past input");
                }
                let b = compressed[i];
                i += 1;
                lit_len += usize::from(b);
                if b != 255 {
                    break;
                }
            }
        }

        // Copy literals.
        if i + lit_len > compressed.len() {
            return Err("literal data extends past input");
        }
        out.extend_from_slice(&compressed[i..i + lit_len]);
        i += lit_len;

        // If we've consumed all input, this was the last token (literal-only).
        if i >= compressed.len() {
            break;
        }

        // Match offset (2 bytes LE).
        if i + 2 > compressed.len() {
            return Err("match offset extends past input");
        }
        let offset = usize::from(compressed[i]) | (usize::from(compressed[i + 1]) << 8);
        i += 2;
        if offset == 0 {
            return Err("zero match offset");
        }
        if offset > out.len() {
            return Err("match offset beyond output");
        }

        // Match length.
        let mut match_len = usize::from(token & 0x0F);
        if match_len == 15 {
            loop {
                if i >= compressed.len() {
                    return Err("match length extends past input");
                }
                let b = compressed[i];
                i += 1;
                match_len += usize::from(b);
                if b != 255 {
                    break;
                }
            }
        }
        match_len += MIN_MATCH;

        // Copy (with overlap).
        let start = out.len() - offset;
        for k in 0..match_len {
            let b = out[start + k];
            out.push(b);
        }
    }

    if expected_len > 0 && out.len() != expected_len {
        return Err("decoded length mismatch");
    }
    Ok(out)
}

/// Write a token byte + literal-length extension for a literal-only
/// sequence (no match). Used for the trailing literal run.
fn write_token_literals(out: &mut Vec<u8>, lit_len: usize) {
    let lit_code = lit_len.min(15);
    out.push((lit_code as u8) << 4);
    // LZ4 spec: extension bytes ONLY present when code nibble == 15.
    if lit_len >= 15 {
        write_length_ext(out, lit_len - 15);
    }
}

/// Write a variable-length extension. ALWAYS writes at least one byte
/// (the final 0 if remainder is 0). Caller MUST only call this when
/// the code nibble == 15.
fn write_length_ext(out: &mut Vec<u8>, mut remaining: usize) {
    while remaining >= 255 {
        out.push(255);
        remaining -= 255;
    }
    out.push(remaining as u8);
}

/// 5-byte hash into the 4096-entry table (`LZ4_hash5`, little-endian
/// variant — the byte order is fixed so every machine produces the same
/// table and therefore the same output).
fn hash5(data: &[u8], pos: usize) -> usize {
    let mut seq = [0u8; 8];
    seq.copy_from_slice(&data[pos..pos + 8]);
    let sequence = u64::from_le_bytes(seq);
    (((sequence << 24).wrapping_mul(PRIME_5_BYTES)) >> (64 - HASH_LOG)) as usize
}
/// Count equal bytes at `pin`/`pmatch` while `pin < limit` (`LZ4_count`).
/// Compares 8 bytes at a time, then resolves the first differing byte
/// via the low bit of the XOR (little-endian loads).
fn count_equal(data: &[u8], mut pin: usize, mut pmatch: usize, limit: usize) -> usize {
    let start = pin;
    while pin + 8 <= limit {
        let a = u64::from_le_bytes(data[pin..pin + 8].try_into().unwrap());
        let b = u64::from_le_bytes(data[pmatch..pmatch + 8].try_into().unwrap());
        if a != b {
            return (pin - start) + ((a ^ b).trailing_zeros() / 8) as usize;
        }
        pin += 8;
        pmatch += 8;
    }
    while pin < limit && data[pin] == data[pmatch] {
        pin += 1;
        pmatch += 1;
    }
    pin - start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_round_trip_empty() {
        let input = b"";
        let compressed = compress_block(input);
        let decompressed = decompress_block(&compressed, 0).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn block_round_trip_short() {
        let input = b"hi";
        let compressed = compress_block(input);
        let decompressed = decompress_block(&compressed, input.len()).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn block_round_trip_text() {
        let input = b"hello world hello world hello world hello world hello world";
        let compressed = compress_block(input);
        assert!(
            compressed.len() < input.len(),
            "should compress: {} vs {}",
            compressed.len(),
            input.len()
        );
        let decompressed = decompress_block(&compressed, input.len()).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn block_round_trip_repetitive() {
        let input: Vec<u8> = (0..8192).map(|i| b'a' + ((i % 4) as u8)).collect();
        let compressed = compress_block(&input);
        assert!(compressed.len() < input.len() / 4);
        let decompressed = decompress_block(&compressed, input.len()).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn block_round_trip_incompressible() {
        let input: Vec<u8> = (0..4096u32)
            .map(|i| i.wrapping_mul(2654435761) as u8)
            .collect();
        let compressed = compress_block(&input);
        let decompressed = decompress_block(&compressed, input.len()).expect("decode");
        assert_eq!(decompressed, input);
    }

    /// Regression: the pre-port encoder bailed to literal-only mode for
    /// the whole file when the first 256 positions looked incompressible
    /// (e.g. a TTF table directory), forfeiting every later match. A
    /// pseudo-random prefix must not stop the compressor from finding the
    /// matches that follow it.
    #[test]
    fn incompressible_prefix_does_not_disable_matching() {
        let mut input: Vec<u8> = (0..4096u32)
            .map(|i| i.wrapping_mul(2654435761) as u8)
            .collect();
        input.extend_from_slice(&vec![b'z'; 100_000]);
        let compressed = compress_block(&input);
        assert!(
            compressed.len() < input.len() / 2,
            "100k run after 4k noise must compress: {} vs {}",
            compressed.len(),
            input.len()
        );
        let decompressed = decompress_block(&compressed, input.len()).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn block_round_trip_long_literals() {
        // Literal run > 15 bytes needs extension.
        let input: Vec<u8> = (0..200).map(|i| (i % 251) as u8).collect();
        let compressed = compress_block(&input);
        let decompressed = decompress_block(&compressed, input.len()).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn block_cross_compat_with_hc_encoder() {
        // The fast encoder's output should decode via the HC decoder
        // and vice versa (same block format).
        let input = b"the quick brown fox jumps over the lazy dog. ".repeat(10);
        let fast_compressed = compress_block(&input);
        // HC encoder produces the same format.
        let hc_compressed = crate::hc::compress(&input);
        // Both should decode via our block decoder.
        let from_fast = decompress_block(&fast_compressed, input.len()).expect("decode fast");
        let from_hc = decompress_block(&hc_compressed, input.len()).expect("decode hc");
        assert_eq!(from_fast, input);
        assert_eq!(from_hc, input);
    }

    #[test]
    fn block_decoder_rejects_zero_offset() {
        // token=0x00 (0 literal, 0 match code), no literal, offset=0.
        let bad = [0x00u8, 0x00, 0x00];
        let result = decompress_block(&bad, 10);
        assert!(result.is_err());
    }

    /// Regression test for the LimniFS-discovered bug where the LZ4
    /// block encoder failed to write an extension byte when the code
    /// nibble was exactly 15 (match length 19, or literal length 15).
    /// The decoder expected at least one extension byte but the encoder
    /// wrote none, corrupting the stream.
    #[test]
    fn block_extension_byte_at_boundary_15() {
        // Construct an input that produces a match of length exactly 19
        // (MIN_MATCH + 15 = 19). Match code = 15 → extension byte
        // required.
        let pattern: Vec<u8> = (0..19u8).collect();
        let mut input = pattern.clone();
        input.extend_from_slice(&pattern);
        input.extend_from_slice(b"trailing unique data to pad length");
        let compressed = compress_block(&input);
        let decompressed = decompress_block(&compressed, input.len()).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn block_extension_byte_for_long_literal_15() {
        // Literal run of exactly 15 bytes → lit_code = 15 → extension
        // byte required.
        let input: Vec<u8> = (0..15)
            .collect::<Vec<u8>>()
            .iter()
            .cycle()
            .take(64)
            .copied()
            .collect();
        let compressed = compress_block(&input);
        let decompressed = decompress_block(&compressed, input.len()).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn block_handles_large_input_64_bytes() {
        // Specifically tests the LimniFS threshold: inputs ≥ 64 bytes.
        let input: Vec<u8> = (0..64u32)
            .map(|i| i.wrapping_mul(2654435761) as u8)
            .collect();
        let compressed = compress_block(&input);
        let decompressed = decompress_block(&compressed, input.len()).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn block_handles_very_long_match() {
        // Match > 255 bytes (triggers multiple 0xFF extension bytes).
        let pattern: Vec<u8> = (0..100).map(|i| (i % 251) as u8).collect();
        let mut input = pattern.clone();
        input.extend_from_slice(&pattern);
        input.extend_from_slice(&pattern);
        let compressed = compress_block(&input);
        let decompressed = decompress_block(&compressed, input.len()).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn block_round_trip_128_bytes_various_patterns() {
        // Comprehensive test at the 128-byte boundary with 5 different
        // patterns that historically triggered the extension byte bug.
        let patterns: Vec<Vec<u8>> = vec![
            vec![0u8; 128],                                   // all-zero
            (0..128).map(|i| (i % 7) as u8).collect(),        // periodic
            (0..128).map(|i| (i * 31 % 251) as u8).collect(), // pseudo-random
            b"the quick brown fox jumps over the lazy dog. "
                .repeat(3)
                .to_vec(), // text
            (0..128)
                .map(|i| if i % 100 < 50 { 0u8 } else { 0xFF })
                .collect(), // binary
        ];
        for (idx, input) in patterns.iter().enumerate() {
            let compressed = compress_block(input);
            let decompressed = decompress_block(&compressed, input.len())
                .unwrap_or_else(|e| panic!("pattern {idx} decode failed: {e}"));
            assert_eq!(
                decompressed.as_slice(),
                input.as_slice(),
                "pattern {idx} mismatch"
            );
        }
    }
}
