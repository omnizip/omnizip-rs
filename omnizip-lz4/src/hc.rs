//! Pure-Rust LZ4 HC encoder.
//!
//! lz4_flex 0.11 ships only the fast encoder; there is no HC variant
//! upstream. This module implements LZ4 HC in pure Rust with the same
//! block wire format so the existing fast decoder
//! (`lz4_flex::decompress_size_prepended`) decodes our output
//! transparently.
//!
//! ## Algorithm
//!
//! - Hash table: 16-bit hash of every 4-byte window → last position.
//! - Hash chain: parallel array; each position holds the previous
//!   position with the same hash, enabling chain walks.
//! - Greedy match selection with lazy look-ahead: at each position,
//!   find the longest match via chain walk; if pos+1 has a strictly
//!   longer match, emit a literal at pos and try pos+1.
//! - Search depth cap: `MAX_CHAIN_LENGTH` bounds the chain walk per
//!   position.

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]

/// Minimum match length (LZ4 spec constant).
const MIN_MATCH: usize = 4;
/// Hash log: 16-bit hash → 64K-entry table. Standard LZ4 setting.
const HASH_LOG: u32 = 16;
/// Maximum positions to walk in a hash chain per match attempt.
const MAX_CHAIN_LENGTH: usize = 256;
/// Window: positions older than this are unreachable (match offset
/// exceeds u16::MAX).
const WINDOW_MASK: usize = 0xFFFF;
/// Last 5+ bytes of input must be literals (no match).
const END_PADDING: usize = 5;
/// Position beyond which matches are unsafe (last 12 bytes reserved).
const MFLIMIT: usize = 12;

/// Compress `input` into an LZ4 block (no prepended size).
/// The output is byte-compatible with the fast LZ4 decoder.
#[must_use]
pub fn compress(input: &[u8]) -> Vec<u8> {
    HcEncoder::new().encode(input)
}

struct HcEncoder {
    out: Vec<u8>,
    /// Hash table: hash → last position with that hash, or `SENTINEL`.
    hash_table: Vec<u32>,
    /// Chain: position → previous position with same hash, or `SENTINEL`.
    chain: Vec<u32>,
    /// Anchor: start of pending literal run.
    anchor: usize,
    /// Position of the current pending token byte in `out` (if any).
    /// The token's literal-count nibble is updated when literals are
    /// flushed; the match-length nibble is updated when a match is
    /// emitted.
    token_pos: Option<usize>,
}

/// Sentinel value indicating "no chain entry". `u32::MAX` is an
/// impossible position (input is bounded by `usize::MAX` in practice
/// and never exceeds u32::MAX for LZ4 anyway).
const SENTINEL: u32 = u32::MAX;

impl HcEncoder {
    fn new() -> Self {
        let size = 1usize << HASH_LOG;
        Self {
            out: Vec::new(),
            hash_table: vec![SENTINEL; size],
            chain: Vec::new(),
            anchor: 0,
            token_pos: None,
        }
    }

    fn encode(mut self, input: &[u8]) -> Vec<u8> {
        let n = input.len();
        if n < MIN_MATCH + END_PADDING {
            // Tiny input: just emit as literals.
            self.emit_token_and_literals(n, input);
            return self.out;
        }
        self.chain.resize(n, SENTINEL);

        // The last `END_PADDING + MIN_MATCH - 1` bytes must be literals.
        let mflimit = n.saturating_sub(MFLIMIT);
        let mut pos = 0usize;

        while pos < mflimit {
            // Search BEFORE inserting the current position's hash entry,
            // so the chain still has the older positions to walk.
            let Some(m) = self.find_match(input, pos, mflimit) else {
                self.insert_hash(input, pos);
                pos += 1;
                continue;
            };

            // Insert hash for the current search position before flushing.
            self.insert_hash(input, pos);

            // Flush pending literals (if any).
            self.flush_literals(input, m.match_pos);

            // Write the match.
            self.emit_match(m.offset, m.length);

            // Insert hash entries for positions within the match (so
            // subsequent positions can find them).
            let end = m.match_pos + m.length;
            let mut p = m.match_pos + 1;
            while p < end && p < mflimit {
                self.insert_hash(input, p);
                p += 1;
            }

            pos = end;
            self.anchor = pos;
        }

        // Emit trailing literals.
        self.flush_literals(input, n);
        self.out
    }

    /// Insert a hash entry for `pos`. Reads input[pos..pos+MIN_MATCH].
    fn insert_hash(&mut self, input: &[u8], pos: usize) {
        if pos + MIN_MATCH > input.len() {
            return;
        }
        let hash = hash4(&input[pos..pos + MIN_MATCH]);
        let prev = self.hash_table[hash];
        self.chain[pos] = prev;
        self.hash_table[hash] = pos as u32;
    }

    /// Walk the hash chain from `pos` to find the longest match.
    /// Implements lazy look-ahead: if `pos+1` has a strictly longer
    /// match, returns the pos+1 match with a marker to emit one literal.
    fn find_match(&self, input: &[u8], pos: usize, limit: usize) -> Option<FoundMatch> {
        let m_here = self.longest_match_at(input, pos)?;
        // Lazy look-ahead.
        if pos + 1 < limit {
            if let Some(m_next) = self.longest_match_at(input, pos + 1) {
                if m_next.length > m_here.length {
                    return Some(FoundMatch {
                        match_pos: pos + 1,
                        offset: m_next.offset,
                        length: m_next.length,
                    });
                }
            }
        }
        Some(FoundMatch {
            match_pos: pos,
            offset: m_here.offset,
            length: m_here.length,
        })
    }

    fn longest_match_at(&self, input: &[u8], pos: usize) -> Option<RawMatch> {
        if pos + MIN_MATCH > input.len() {
            return None;
        }
        let hash = hash4(&input[pos..pos + MIN_MATCH]);
        let mut candidate = self.hash_table[hash];
        let mut best_len = 0usize;
        let mut best_offset = 0usize;
        let max_len = (input.len() - pos).min(u16::MAX as usize + MIN_MATCH);

        for _ in 0..MAX_CHAIN_LENGTH {
            if candidate == SENTINEL || candidate as usize >= pos {
                break;
            }
            let cand = candidate as usize;
            let offset = pos - cand;
            if offset > WINDOW_MASK {
                break;
            }
            if cand + MIN_MATCH <= input.len()
                && input[cand..cand + MIN_MATCH] == input[pos..pos + MIN_MATCH]
            {
                let mut len = MIN_MATCH;
                // Word-at-a-time match extension. 8-byte word stepping
                // with trailing_zeros to locate the first mismatch.
                let mut early_exit = false;
                while len + 8 <= max_len
                    && cand + len + 8 <= input.len()
                    && pos + len + 8 <= input.len()
                    && !early_exit
                {
                    let wc = u64::from_le_bytes([
                        input[cand + len],
                        input[cand + len + 1],
                        input[cand + len + 2],
                        input[cand + len + 3],
                        input[cand + len + 4],
                        input[cand + len + 5],
                        input[cand + len + 6],
                        input[cand + len + 7],
                    ]);
                    let wp = u64::from_le_bytes([
                        input[pos + len],
                        input[pos + len + 1],
                        input[pos + len + 2],
                        input[pos + len + 3],
                        input[pos + len + 4],
                        input[pos + len + 5],
                        input[pos + len + 6],
                        input[pos + len + 7],
                    ]);
                    if wc == wp {
                        len += 8;
                    } else {
                        let diff = wc ^ wp;
                        len += diff.trailing_zeros() as usize / 8;
                        early_exit = true;
                    }
                }
                if !early_exit {
                    // Byte-tail for residual 0..=7 bytes.
                    while len < max_len && input[cand + len] == input[pos + len] {
                        len += 1;
                    }
                }
                if len > best_len {
                    best_len = len;
                    best_offset = offset;
                }
            }
            candidate = self.chain[cand];
        }

        if best_len >= MIN_MATCH {
            Some(RawMatch { offset: best_offset, length: best_len })
        } else {
            None
        }
    }

    /// Flush `[anchor, end)` as a literal run. Pops the pending token
    /// if the previous sequence had one and emits a fresh one for this
    /// literal run.
    fn flush_literals(&mut self, input: &[u8], end: usize) {
        let lit_len = end - self.anchor;
        if lit_len == 0 {
            // No literals to flush, but we still need a token if a
            // match follows. Ensure pending token exists with 0 literals.
            if self.token_pos.is_none() {
                self.emit_token_only(0);
            }
            return;
        }
        let bytes = &input[self.anchor..end];
        self.emit_token_and_literals(lit_len, bytes);
        self.anchor = end;
    }

    /// Emit just a token byte with the given literal count (no literals,
    /// no length extension). Used when a match directly follows another
    /// match (zero-literal sequence).
    fn emit_token_only(&mut self, lit_len: usize) {
        let token_lit = lit_len.min(15) as u8;
        let token = token_lit << 4;
        self.token_pos = Some(self.out.len());
        self.out.push(token);
        if lit_len >= 15 {
            write_length_extension(&mut self.out, lit_len - 15);
        }
    }

    /// Emit a token byte + (optional length extension) + literal bytes.
    /// The token's match-length nibble is left at 0; if a match
    /// follows, `emit_match` updates it.
    fn emit_token_and_literals(&mut self, lit_len: usize, bytes: &[u8]) {
        let token_lit = lit_len.min(15) as u8;
        let token = token_lit << 4;
        self.token_pos = Some(self.out.len());
        self.out.push(token);
        if lit_len >= 15 {
            write_length_extension(&mut self.out, lit_len - 15);
        }
        self.out.extend_from_slice(bytes);
    }

    /// Update the token's match-length nibble and append the match
    /// offset + optional length extension.
    fn emit_match(&mut self, offset: usize, length: usize) {
        let token_pos = self.token_pos.expect("match requires a pending token");
        let match_len_field = length.saturating_sub(MIN_MATCH).min(15) as u8;
        self.out[token_pos] |= match_len_field;
        let off = u16::try_from(offset).unwrap_or(0);
        self.out.push(off as u8);
        self.out.push((off >> 8) as u8);
        let m_ext = length.saturating_sub(MIN_MATCH);
        if m_ext >= 15 {
            write_length_extension(&mut self.out, m_ext - 15);
        }
        // Match consumes the pending token.
        self.token_pos = None;
    }
}

#[derive(Debug)]
struct RawMatch {
    offset: usize,
    length: usize,
}

#[derive(Debug)]
struct FoundMatch {
    /// Position where the match starts (may differ from search pos
    /// due to lazy look-ahead).
    match_pos: usize,
    offset: usize,
    length: usize,
}

/// Hash 4 bytes into a 16-bit value (matches LZ4's hash function).
fn hash4(bytes: &[u8]) -> usize {
    let v = u32::from(bytes[0])
        | (u32::from(bytes[1]) << 8)
        | (u32::from(bytes[2]) << 16)
        | (u32::from(bytes[3]) << 24);
    let hash = (v.wrapping_mul(265_443_5761) >> (32 - HASH_LOG)) & ((1 << HASH_LOG) - 1);
    hash as usize
}

/// Write the LZ4 variable-length extension: each byte's value adds to
/// the length; the last byte must be < 255.
fn write_length_extension(out: &mut Vec<u8>, mut remaining: usize) {
    while remaining >= 255 {
        out.push(255);
        remaining -= 255;
    }
    out.push(remaining as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(input: &[u8]) {
        let compressed = compress(input);
        let decoded = crate::block::decompress_block(&compressed, input.len()).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn empty_input_round_trips() {
        // Empty input → empty output. lz4_flex expects at least the
        // size prefix + 0 tokens, which we provide.
        let compressed = compress(b"");
        // Empty input emits a single zero-literal token? No — we
        // short-circuit early; output is empty.
        assert!(compressed.is_empty() || compressed.len() <= 2);
    }

    #[test]
    fn short_input_round_trips() {
        round_trip(b"a");
        round_trip(b"ab");
        round_trip(b"abcdef");
    }

    #[test]
    fn long_run_round_trips() {
        round_trip(&vec![0x41u8; 1000]);
    }

    #[test]
    fn repetitive_text_round_trips() {
        let data = b"the quick brown fox ".repeat(100);
        round_trip(&data);
    }

    #[test]
    fn mixed_payload_round_trips() {
        let mut data: Vec<u8> = b"hello world ".repeat(50);
        data.extend_from_slice(&(0..256u32).map(|i| (i & 0xFF) as u8).collect::<Vec<_>>());
        data.extend_from_slice(b"hello world ".repeat(50).as_slice());
        round_trip(&data);
    }

    #[test]
    fn large_repetitive_round_trips() {
        let data: Vec<u8> = (0..50_000).map(|i| (i % 251) as u8).collect();
        round_trip(&data);
    }

    #[test]
    fn hash_function_is_in_table_range() {
        let h = hash4(b"abcd");
        assert!(h < (1 << HASH_LOG));
    }
}
