//! LZ4 block-format encoder + decoder (pure-Rust, from spec).
//!
//! Implements the LZ4 block format described at
//! <https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md>.
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
/// Maximum match length that fits in a single token nibble (15 + 3 = 18,
/// but the extension allows up to 2^32 - 1 in practice).
const MAX_MATCH: usize = 65535;
/// Hash table size.
const HASH_LOG: u32 = 16;
const HASH_SIZE: usize = 1 << HASH_LOG;

/// Compress `input` into an LZ4 block (no size prefix, no frame wrapper).
/// Fast mode: single-probe hash, no chain, no lazy look-ahead.
#[must_use]
pub fn compress_block(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        // Single token with literal length 0, no match.
        return vec![0u8];
    }
    if input.len() < MIN_MATCH + 5 {
        // Too short for matches — emit as a single literal-only token.
        let mut out = Vec::with_capacity(input.len() + 4);
        write_token_literals(&mut out, input.len());
        out.extend_from_slice(input);
        return out;
    }

    let mut hash_table = vec![0u32; HASH_SIZE];
    let mut out = Vec::with_capacity(input.len());
    let mut anchor = 0usize;
    let mut pos = 0usize;
    let last_match_start = input.len().saturating_sub(MIN_MATCH);

    while pos < last_match_start {
        let h = hash4(input, pos);
        let candidate = hash_table[h] as usize;
        hash_table[h] = pos as u32;

        if candidate > 0 && candidate < pos && pos - candidate <= 65535 {
            if input[candidate..candidate + MIN_MATCH] == input[pos..pos + MIN_MATCH] {
                // Extend match.
                let mut mlen = MIN_MATCH;
                while pos + mlen < input.len()
                    && mlen < MAX_MATCH
                    && input[candidate + mlen] == input[pos + mlen]
                {
                    mlen += 1;
                }

                // Emit literals + match.
                let lit_len = pos - anchor;
                let offset = pos - candidate;
                // Token: (lit_code << 4) | m_code.
                let lit_code = lit_len.min(15);
                let m_code = (mlen - MIN_MATCH).min(15);
                out.push(((lit_code as u8) << 4) | (m_code as u8));

                // Literal length extension.
                write_length_ext(&mut out, lit_len.saturating_sub(15));

                // Literal bytes.
                out.extend_from_slice(&input[anchor..pos]);

                // Offset (2 bytes LE).
                out.extend_from_slice(&(offset as u16).to_le_bytes());

                // Match length extension.
                write_length_ext(&mut out, (mlen - MIN_MATCH).saturating_sub(15));

                // Insert hash for a few positions inside the match.
                let end = pos + mlen;
                let mut ip = pos + 1;
                while ip < end.min(last_match_start) {
                    let h2 = hash4(input, ip);
                    hash_table[h2] = ip as u32;
                    ip += 1;
                }
                pos = end;
                anchor = pos;
                continue;
            }
        }
        pos += 1;
    }

    // Emit trailing literals.
    let lit_len = input.len() - anchor;
    write_token_literals(&mut out, lit_len);
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
    write_length_ext(out, lit_len.saturating_sub(15));
}

/// Write a variable-length extension: 0xFF bytes until remaining < 255,
/// then one final byte with the remainder.
fn write_length_ext(out: &mut Vec<u8>, mut remaining: usize) {
    while remaining >= 255 {
        out.push(255);
        remaining -= 255;
    }
    if remaining > 0 {
        out.push(remaining as u8);
    }
}

/// 4-byte hash into a 16-bit table.
fn hash4(data: &[u8], pos: usize) -> usize {
    let val = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
    ((val.wrapping_mul(2654435761) >> (32 - HASH_LOG)) as usize) & (HASH_SIZE - 1)
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
        assert!(compressed.len() < input.len(), "should compress: {} vs {}", compressed.len(), input.len());
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
        let input: Vec<u8> = (0..4096u32).map(|i| i.wrapping_mul(2654435761) as u8).collect();
        let compressed = compress_block(&input);
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
}
