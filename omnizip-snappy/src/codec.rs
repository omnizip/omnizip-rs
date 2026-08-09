//! Pure-Rust Snappy encoder (from spec).
//!
//! Implements the Snappy framing format described at
//! <https://github.com/google/snappy/blob/main/format_description.txt>.
//!
//! ## Wire format
//!
//! ```text
//! varint(uncompressed_length)
//! tag_stream  (variable — tag byte per literal/match)
//! ```
//!
//! Tag byte low 2 bits select the element type:
//! - `00` = LITERAL: high 6 bits encode literal length (with extension).
//! - `01` = `COPY_1`: 1-byte offset (12-bit offset, 3-bit length-4).
//! - `10` = `COPY_2`: 2-byte offset (16-bit offset, 6-bit length-1).
//! - `11` = `COPY_4`: 4-byte offset (32-bit offset, 6-bit length-1).
//!
//! ## Match finder
//!
//! Single-probe hash table (no chain). Snappy is optimised for speed
//! over ratio. Min match = 4, max match = 64, window = 32 KiB.

#![forbid(unsafe_code)]

const MIN_MATCH: usize = 4;
const MAX_MATCH: usize = 64;
const WINDOW_SIZE: usize = 32 * 1024;

/// Encode `input` as a Snappy frame. Returns the framed bytes.
#[must_use]
pub fn encode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + 8);
    write_varint(&mut out, input.len() as u64);

    let mut mf = HashTable::new();
    let mut i = 0;
    while i < input.len() {
        // Try to find a match at i.
        if let Some((dist, len)) = mf.find_match(input, i) {
            emit_copy(&mut out, dist, len);
            // Insert positions covered by the match. Snappy inserts
            // only every other position to amortise hash cost (matches
            // the C reference's `EmitCopy` + skip pattern).
            let mut k = i;
            let end = i + len;
            while k < end {
                mf.insert(input, k);
                k += 1;
            }
            i = end;
        } else {
            // Literal run: scan until we find a match.
            let lit_start = i;
            i += 1;
            while i < input.len() {
                if let Some((_, _)) = mf.find_match(input, i) {
                    break;
                }
                mf.insert(input, i - 1);
                i += 1;
                if i - lit_start >= 60 {
                    break; // literal length cap before tag extension
                }
            }
            // Final insert for the position before the literal end.
            if i > lit_start && i - 1 < input.len() {
                mf.insert(input, i - 1);
            }
            let lit_len = i - lit_start;
            emit_literal(&mut out, &input[lit_start..lit_start + lit_len]);
        }
    }
    out
}

/// Decode a Snappy frame. Returns the uncompressed bytes.
///
/// # Errors
///
/// Returns `&'static str` on malformed input.
pub fn decode(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let (uncompressed_len, bytes_consumed) =
        read_varint(input).ok_or("invalid varint length preamble")?;
    let uncompressed_len = uncompressed_len as usize;
    let mut out = Vec::with_capacity(uncompressed_len);
    let mut i = bytes_consumed;

    while i < input.len() && out.len() < uncompressed_len {
        let tag = input[i];
        i += 1;
        let tag_type = tag & 0b11;
        match tag_type {
            0 => {
                // LITERAL.
                let (lit_len, consumed) = decode_literal_length(tag, &input[i..])?;
                i += consumed;
                let end = i + lit_len;
                if end > input.len() {
                    return Err("literal extends past end of input");
                }
                out.extend_from_slice(&input[i..end]);
                i = end;
            }
            1 => {
                // COPY_1: 1-byte offset. Wire stores RAW distance
                // (high 3 bits in tag, low 8 bits in next byte).
                let len = (usize::from(tag >> 2) & 0b111) + 4;
                let dist = (usize::from(tag >> 5) << 8) | usize::from(input[i]);
                i += 1;
                if dist == 0 || dist > out.len() {
                    return Err("invalid copy distance");
                }
                copy_overlap(&mut out, dist, len);
            }
            2 => {
                // COPY_2: 2-byte offset. Wire stores RAW distance LE.
                let len = (usize::from(tag >> 2)) + 1;
                let dist = usize::from(input[i]) | (usize::from(input[i + 1]) << 8);
                i += 2;
                if dist == 0 || dist > out.len() {
                    return Err("invalid copy distance");
                }
                copy_overlap(&mut out, dist, len);
            }
            3 => {
                // COPY_4: 4-byte offset. Wire stores RAW distance LE.
                let len = (usize::from(tag >> 2)) + 1;
                let dist = usize::from(input[i])
                    | (usize::from(input[i + 1]) << 8)
                    | (usize::from(input[i + 2]) << 16)
                    | (usize::from(input[i + 3]) << 24);
                i += 4;
                if dist == 0 || dist > out.len() {
                    return Err("invalid copy distance");
                }
                copy_overlap(&mut out, dist, len);
            }
            _ => unreachable!("2-bit tag"),
        }
    }

    if out.len() != uncompressed_len {
        return Err("decoded length mismatch");
    }
    Ok(out)
}

/// Hash table for Snappy's single-probe match finder.
struct HashTable {
    head: Vec<u32>,
}

impl HashTable {
    fn new() -> Self {
        Self {
            head: vec![0; 1 << 14],
        }
    }

    fn hash(data: &[u8], pos: usize) -> usize {
        if pos + 4 > data.len() {
            return 0;
        }
        let word = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        (word.wrapping_mul(0x9E37_79B1) >> (32 - 14)) as usize
    }

    fn insert(&mut self, data: &[u8], pos: usize) {
        if pos + 4 > data.len() {
            return;
        }
        let h = Self::hash(data, pos);
        self.head[h] = pos as u32;
    }

    fn find_match(&self, data: &[u8], pos: usize) -> Option<(usize, usize)> {
        if pos + MIN_MATCH > data.len() {
            return None;
        }
        let h = Self::hash(data, pos);
        let candidate = self.head[h] as usize;
        if candidate == 0 || candidate >= pos {
            return None;
        }
        let dist = pos - candidate;
        if dist > WINDOW_SIZE {
            return None;
        }
        if data[candidate..candidate + MIN_MATCH] != data[pos..pos + MIN_MATCH] {
            return None;
        }
        let mut len = MIN_MATCH;
        while len < MAX_MATCH && pos + len < data.len() && data[candidate + len] == data[pos + len]
        {
            len += 1;
        }
        Some((dist, len))
    }
}

/// Emit a literal run.
fn emit_literal(out: &mut Vec<u8>, lit: &[u8]) {
    let n = lit.len();
    if n <= 60 {
        out.push(((n - 1) << 2) as u8);
    } else if n < 256 {
        out.push(60 << 2);
        out.push((n - 1) as u8);
    } else if n < 65536 {
        out.push(61 << 2);
        out.extend_from_slice(&((n - 1) as u16).to_le_bytes());
    } else if n < 1 << 24 {
        out.push(62 << 2);
        let v = (n - 1) as u32;
        out.extend_from_slice(&v.to_le_bytes()[..3]);
    } else {
        out.push(63 << 2);
        out.extend_from_slice(&((n - 1) as u32).to_le_bytes());
    }
    out.extend_from_slice(lit);
}

/// Emit a copy command. Picks the smallest tag that fits.
///
/// ## Wire format
///
/// Per the Snappy framing format (matching upstream `snap`): distances
/// are stored RAW in the wire bytes (not `distance - 1`).
fn emit_copy(out: &mut Vec<u8>, dist: usize, len: usize) {
    // COPY_1 handles dist 1..=2047 and len 4..=11.
    if dist <= 2047 && len <= 11 {
        let tag = ((dist >> 8) << 5) | (((len - 4) & 0b111) << 2) | 0b01;
        out.push(tag as u8);
        out.push((dist & 0xFF) as u8);
        return;
    }
    // COPY_2 handles dist 1..=65535 and len 1..=64.
    // Emit in 64-byte chunks.
    let dist_le = dist as u16;
    let mut remaining = len;
    while remaining > 0 {
        let chunk = remaining.min(64);
        let tag = ((chunk - 1) as u8) << 2 | 0b10;
        out.push(tag);
        out.extend_from_slice(&dist_le.to_le_bytes());
        remaining -= chunk;
    }
}

/// Decode the literal length from a LITERAL tag byte + extension bytes.
fn decode_literal_length(tag: u8, rest: &[u8]) -> Result<(usize, usize), &'static str> {
    let n = usize::from(tag >> 2);
    // 0..=59 → literal length 1..=60 (no extension).
    // 60..=63 → extension: 1, 2, 3, or 4 bytes follow.
    if n < 60 {
        return Ok((n + 1, 0));
    }
    let extension_bytes = 1usize << (n - 60);
    if rest.len() < extension_bytes {
        return Err("literal length extends past end of input");
    }
    let mut len = 0usize;
    for i in 0..extension_bytes {
        len |= usize::from(rest[i]) << (8 * i);
    }
    Ok((len + 1, extension_bytes))
}

/// Overlapping copy (distance < length is valid in Snappy).
fn copy_overlap(out: &mut Vec<u8>, dist: usize, len: usize) {
    let start = out.len() - dist;
    for k in 0..len {
        let b = out[start + k];
        out.push(b);
    }
}

/// Write `value` as a varint (LSB-first, 7 bits per byte, MSB=continuation).
fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Read a varint from the start of `input`. Returns `(value, bytes_consumed)`.
fn read_varint(input: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for (i, &b) in input.iter().enumerate() {
        value |= u64::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_round_trip() {
        for &v in &[
            0u64,
            1,
            127,
            128,
            0xFF,
            0xFFFF,
            0xFFFF_FFFF,
            u32::MAX as u64,
        ] {
            let mut buf = Vec::new();
            write_varint(&mut buf, v);
            let (got, consumed) = read_varint(&buf).expect("read");
            assert_eq!(got, v, "value {v} mismatch");
            assert_eq!(consumed, buf.len());
        }
    }

    #[test]
    fn encode_empty_input() {
        let out = encode(&[]);
        // Just the length varint (1 byte = 0).
        assert_eq!(out, vec![0u8]);
    }

    #[test]
    fn decode_empty_input() {
        let out = decode(&[0u8]).expect("decode");
        assert!(out.is_empty());
    }

    #[test]
    fn round_trip_incompressible() {
        let input: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(2654435761) % 251) as u8)
            .collect();
        let encoded = encode(&input);
        let decoded = decode(&encoded).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn round_trip_repetitive() {
        let input: Vec<u8> = (0..4096).map(|i| b'a' + ((i % 4) as u8)).collect();
        let encoded = encode(&input);
        assert!(
            encoded.len() < input.len() / 2,
            "repetitive input should compress: {} vs {}",
            encoded.len(),
            input.len()
        );
        let decoded = decode(&encoded).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn round_trip_text() {
        let input = b"the quick brown fox jumps over the lazy dog. ".repeat(50);
        let encoded = encode(&input);
        let decoded = decode(&encoded).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn decode_rejects_invalid_copy() {
        // Uncompressed_len=5; literal gives 1 byte; COPY_4 needs
        // dist=1000 but output is only 1 byte → reject.
        let mut bad = vec![5u8]; // varint: length = 5
        bad.push(0b0000_0000); // literal tag: length = 1
        bad.push(b'X');
        bad.push(0x0F); // COPY_4 tag, len=4
        bad.push(0xE8); // dist = 1000 (raw LE)
        bad.push(0x03);
        bad.push(0x00);
        bad.push(0x00);
        let result = decode(&bad);
        assert!(result.is_err(), "out-of-range distance should be rejected");
    }

    #[test]
    fn copy_overlap_handles_overlap_correctly() {
        // Snappy allows dist < len (run-length encoding via overlap).
        let mut out = vec![b'A'];
        copy_overlap(&mut out, 1, 5);
        assert_eq!(out, b"AAAAAA");
    }

    #[test]
    fn emit_literal_picks_smallest_tag() {
        let mut out = Vec::new();
        // Length 1-60: 1-byte tag.
        emit_literal(&mut out, b"hi");
        assert_eq!(out.len(), 3); // tag + 2 bytes.

        // Length 61-255: 2-byte tag.
        out.clear();
        let lit = vec![b'X'; 100];
        emit_literal(&mut out, &lit);
        assert_eq!(out.len(), 102); // tag + length byte + 100 bytes.

        // Length 256-65535: 3-byte tag.
        out.clear();
        let lit = vec![b'X'; 1000];
        emit_literal(&mut out, &lit);
        assert_eq!(out.len(), 1003); // tag + 2 length bytes + 1000 bytes.
    }
}
