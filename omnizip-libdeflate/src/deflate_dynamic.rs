//! DEFLATE encoder with LZ77 + dynamic Huffman codes (RFC 1951 §3.2.7).
//!
//! Built on top of [`super::deflate_lz77`] (LZ77 + fixed Huffman).
//! Adds a dynamic-Huffman block writer that ships its own optimised
//! Huffman tables in the block header.
//!
//! ## Wire format
//!
//! ```text
//! BFINAL (1 bit) | BTYPE=2 (2 bits)
//! HLIT (5 bits)  | HDIST (5 bits) | HCLEN (4 bits)
//! code length code lengths (HCLEN+4) × 3 bits
//! literal+length code lengths (HLIT+257 entries, run-length encoded)
//! distance code lengths (HDIST+1 entries, run-length encoded)
//! compressed data (Huffman-coded literals/lengths + distances)
//! ```
//!
//! ## Algorithm
//!
//! 1. Run LZ77 to get tokens (reuses [`deflate_lz77::collect_tokens`]).
//! 2. Tally symbol frequencies for literals/lengths and distances.
//! 3. Build canonical Huffman tables via package-merge with 15-bit cap.
//! 4. Run-length encode the code lengths using the spec's 19-symbol
//!    code-length alphabet, build its Huffman table (7-bit cap), emit
//!    everything.
//! 5. Emit the Huffman-coded symbols.

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]

use super::deflate_lz77::{collect_tokens, Lz77Token};
use omnizip_codecs::OmnizipError;

/// Maximum Huffman code length for literal/length and distance codes
/// (RFC 1951 §3.2.7).
const MAX_CODE_LEN: u8 = 15;
/// Maximum code length for the code-length code (RFC 1951 §3.2.7).
const MAX_CL_CODE_LEN: u8 = 7;

/// Order in which code-length codes are emitted (RFC 1951 §3.2.7).
const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Encode `input` as a single RFC 1951 dynamic-Huffman block.
///
/// Returns the raw DEFLATE bytes (no zlib wrapper). The block has
/// `BFINAL=1, BTYPE=2`.
///
/// Returns `None` if the input is too small for dynamic Huffman to
/// pay off — the caller should fall back to fixed or stored.
pub fn deflate_dynamic_huffman(input: &[u8]) -> Result<Option<Vec<u8>>, OmnizipError> {
    if input.len() < 32 {
        return Ok(None);
    }

    let tokens = collect_tokens(input);
    if tokens.is_empty() {
        return Ok(None);
    }

    let mut lit_freqs = [0u32; 286];
    let mut dist_freqs = [0u32; 30];
    lit_freqs[256] = 1; // End-of-block symbol, always emitted once.

    for tok in &tokens {
        match tok {
            Lz77Token::Literal(b) => lit_freqs[usize::from(*b)] += 1,
            Lz77Token::Match { length, distance } => {
                let len_sym = length_to_symbol(*length);
                let dist_sym = distance_to_symbol(*distance);
                lit_freqs[len_sym] += 1;
                dist_freqs[dist_sym] += 1;
            }
        }
    }

    // Build canonical Huffman tables.
    let mut lit_lengths = [0u8; 286];
    let mut dist_lengths = [0u8; 30];
    build_huffman_lengths(&lit_freqs, MAX_CODE_LEN, &mut lit_lengths);
    build_huffman_lengths(&dist_freqs, MAX_CODE_LEN, &mut dist_lengths);

    // Trim trailing zero entries: HLIT = (last used lit/len index) - 256.
    let hlit_count = count_trailing_zeros(&lit_lengths, 257);
    let hdist_count = count_trailing_zeros(&dist_lengths, 1);

    // Concatenate lit+dist lengths and run-length encode using the
    // code-length alphabet (0-15 + 16/17/18 for runs).
    let combined: Vec<u8> = lit_lengths[..hlit_count]
        .iter()
        .chain(dist_lengths[..hdist_count].iter())
        .copied()
        .collect();
    let cl_symbols = run_length_encode(&combined);

    // Build the code-length Huffman table.
    let mut cl_freqs = [0u32; 19];
    for s in cl_symbols.iter() {
        cl_freqs[usize::from(s.symbol)] += 1;
    }
    let mut cl_lengths = [0u8; 19];
    build_huffman_lengths(&cl_freqs, MAX_CL_CODE_LEN, &mut cl_lengths);

    // Determine HCLEN.
    let hclen_count = compute_hclen(&cl_lengths);

    // ---- Emit the block. ----
    let mut writer = BitWriter::new();
    // Block header: BFINAL=1, BTYPE=2 (dynamic).
    writer.write_bits(1, 1); // BFINAL
    writer.write_bits(2, 2); // BTYPE = dynamic

    writer.write_bits((hlit_count - 257) as u64, 5);
    writer.write_bits((hdist_count - 1) as u64, 5);
    writer.write_bits((hclen_count - 4) as u64, 4);

    // Code length code lengths in CL_ORDER.
    for i in 0..hclen_count {
        writer.write_bits(u64::from(cl_lengths[CL_ORDER[i]]), 3);
    }

    // Build canonical codes for the code-length alphabet.
    let cl_codes = canonical_codes(&cl_lengths, MAX_CL_CODE_LEN);

    // Emit the run-length-encoded code lengths.
    let mut prev: u8 = 0;
    for s in cl_symbols.iter() {
        match s.symbol {
            0..=15 => {
                writer.write_huffman_code(cl_codes[s.symbol as usize], cl_lengths[s.symbol as usize]);
                prev = s.symbol;
            }
            16 => {
                writer.write_huffman_code(cl_codes[16], cl_lengths[16]);
                writer.write_bits(u64::from(s.extra), 2);
            }
            17 => {
                writer.write_huffman_code(cl_codes[17], cl_lengths[17]);
                writer.write_bits(u64::from(s.extra), 3);
            }
            18 => {
                writer.write_huffman_code(cl_codes[18], cl_lengths[18]);
                writer.write_bits(u64::from(s.extra), 7);
            }
            _ => unreachable!("invalid CL symbol"),
        }
    }
    let _ = prev;

    // Build canonical codes for lit/len and dist alphabets.
    let lit_codes = canonical_codes(&lit_lengths, MAX_CODE_LEN);
    let dist_codes = canonical_codes(&dist_lengths, MAX_CODE_LEN);

    // Emit the tokens.
    for tok in &tokens {
        match tok {
            Lz77Token::Literal(b) => {
                writer.write_huffman_code(lit_codes[usize::from(*b)], lit_lengths[usize::from(*b)]);
            }
            Lz77Token::Match { length, distance } => {
                let (len_sym, len_extra_bits, len_extra_val) = length_to_symbol_full(*length);
                writer.write_huffman_code(lit_codes[len_sym], lit_lengths[len_sym]);
                if len_extra_bits > 0 {
                    writer.write_bits(u64::from(len_extra_val), len_extra_bits);
                }
                let (dist_sym, dist_extra_bits, dist_extra_val) = distance_to_symbol_full(*distance);
                writer.write_huffman_code(dist_codes[dist_sym], dist_lengths[dist_sym]);
                if dist_extra_bits > 0 {
                    writer.write_bits(u64::from(dist_extra_val), dist_extra_bits);
                }
            }
        }
    }
    // End of block.
    writer.write_huffman_code(lit_codes[256], lit_lengths[256]);

    writer.flush_byte_aligned();
    Ok(Some(writer.finish()))
}

/// Build canonical Huffman code lengths via the package-merge algorithm.
///
/// Build canonical Huffman code lengths via the standard min-heap
/// algorithm, then cap at `max_len` using the zlib CPI approach.
fn build_huffman_lengths(freqs: &[u32], max_len: u8, lengths: &mut [u8]) {
    let symbols: Vec<(u32, usize)> = freqs
        .iter()
        .enumerate()
        .filter(|(_, &f)| f > 0)
        .map(|(i, &f)| (f, i))
        .collect();
    let m = symbols.len();
    if m == 0 {
        return;
    }
    if m == 1 {
        lengths[symbols[0].1] = 1;
        return;
    }

    // Standard Huffman via iterative merge (two-smallest selection).
    struct Node {
        freq: u64,
        leaves: Vec<(usize, u8)>, // (symbol_index, depth_from_root)
    }

    let mut nodes: Vec<Node> = symbols
        .iter()
        .map(|&(f, i)| Node {
            freq: u64::from(f),
            leaves: vec![(i, 0u8)],
        })
        .collect();

    while nodes.len() > 1 {
        nodes.sort_by(|a, b| a.freq.cmp(&b.freq));
        let mut right = nodes.remove(1);
        let mut left = nodes.remove(0);
        for (_, d) in &mut left.leaves { *d += 1; }
        for (_, d) in &mut right.leaves { *d += 1; }
        let merged_freq = left.freq + right.freq;
        let mut merged_leaves = left.leaves;
        merged_leaves.append(&mut right.leaves);
        nodes.push(Node { freq: merged_freq, leaves: merged_leaves });
    }

    // Extract lengths.
    for (sym, depth) in &nodes[0].leaves {
        lengths[*sym] = (*depth).min(255);
    }

    // Length limiting: zlib CPI approach.
    let mut bl_count = [0u32; 256];
    for &l in lengths.iter() {
        if l > 0 {
            bl_count[l as usize] += 1;
        }
    }

    for l in ((max_len as usize + 1)..256).rev() {
        while bl_count[l] > 0 {
            let mut j = l - 1;
            while j > 0 && bl_count[j] == 0 {
                j -= 1;
            }
            if j == 0 {
                break;
            }
            bl_count[j] -= 1;
            bl_count[j + 1] += 2;
            bl_count[l] -= 1;
            bl_count[max_len as usize] += 1;
        }
    }

    // Reassign lengths: highest-frequency symbols get shortest codes.
    let mut sorted_syms: Vec<(usize, u32)> = freqs
        .iter()
        .enumerate()
        .filter(|(_, &f)| f > 0)
        .map(|(i, &f)| (i, f))
        .collect();
    sorted_syms.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut code_idx = 0;
    for len in 1..=max_len as usize {
        for _ in 0..bl_count[len] {
            if code_idx >= sorted_syms.len() {
                break;
            }
            lengths[sorted_syms[code_idx].0] = len as u8;
            code_idx += 1;
        }
    }
}

/// Generate canonical Huffman codes from code lengths (RFC 1951 §3.2.7).
fn canonical_codes(lengths: &[u8], _max_len: u8) -> Vec<u16> {
    let n = lengths.len();
    let mut codes = vec![0u16; n];
    // Count occurrences of each code length.
    let mut bl_count = [0u32; 16];
    for &l in lengths {
        if l > 0 {
            bl_count[usize::from(l)] += 1;
        }
    }
    // Compute first code per length (canonical assignment).
    let mut next_code = [0u16; 16];
    let mut code = 0u16;
    for bits in 1..=15usize {
        code = (code + bl_count[bits - 1] as u16) << 1;
        next_code[bits] = code;
    }
    // Assign codes per symbol in alphabetical order.
    for i in 0..n {
        let l = lengths[i];
        if l > 0 {
            codes[i] = next_code[usize::from(l)];
            next_code[usize::from(l)] += 1;
        }
    }
    codes
}

/// Run-length encode code lengths per RFC 1951 §3.2.7.
struct ClSymbol {
    symbol: u8,
    /// Extra bits value (count for repeats, 0 for literals).
    extra: u8,
}

fn run_length_encode(lengths: &[u8]) -> Vec<ClSymbol> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lengths.len() {
        let cur = lengths[i];
        let mut run = 1;
        while i + run < lengths.len() && lengths[i + run] == cur {
            run += 1;
        }
        if cur == 0 {
            // Use 17 (3-10 zeros) or 18 (11-138 zeros).
            while run >= 11 {
                let take = run.min(138) as u8;
                out.push(ClSymbol { symbol: 18, extra: take - 11 });
                run -= take as usize;
                i += take as usize;
            }
            while run >= 3 {
                let take = run.min(10) as u8;
                out.push(ClSymbol { symbol: 17, extra: take - 3 });
                run -= take as usize;
                i += take as usize;
            }
            while run > 0 {
                out.push(ClSymbol { symbol: 0, extra: 0 });
                run -= 1;
                i += 1;
            }
        } else {
            // Emit one literal, then use 16 (3-6 repeats) for runs.
            out.push(ClSymbol { symbol: cur, extra: 0 });
            i += 1;
            run -= 1;
            while run >= 3 {
                let take = run.min(6) as u8;
                out.push(ClSymbol { symbol: 16, extra: take - 3 });
                run -= take as usize;
                i += take as usize;
            }
            while run > 0 {
                out.push(ClSymbol { symbol: cur, extra: 0 });
                run -= 1;
                i += 1;
            }
        }
    }
    out
}

/// Compute HCLEN: number of code-length codes (≥4, ≤19).
fn compute_hclen(cl_lengths: &[u8; 19]) -> usize {
    for hclen in (1..=19).rev() {
        if cl_lengths[CL_ORDER[hclen - 1]] != 0 {
            return hclen.max(4);
        }
    }
    4
}

/// Number of entries in `lengths` (from the start) up to and including
/// the last non-zero entry, with a minimum of `min_count`.
fn count_trailing_zeros(lengths: &[u8], min_count: usize) -> usize {
    let n = lengths.len();
    for i in (min_count..n).rev() {
        if lengths[i] != 0 {
            return i + 1;
        }
    }
    min_count
}

/// DEFLATE length symbol table (RFC 1951 §3.2.5).
///
/// Returns `(symbol, extra_bits, base_length)`.
fn length_table() -> &'static [(u16, u8, u16)] {
    &[
        (257, 0, 3), (258, 0, 4), (259, 0, 5), (260, 0, 6),
        (261, 0, 7), (262, 0, 8), (263, 0, 9), (264, 0, 10),
        (265, 1, 11), (266, 1, 13), (267, 1, 15), (268, 1, 17),
        (269, 2, 19), (270, 2, 23), (271, 2, 27), (272, 2, 31),
        (273, 3, 35), (274, 3, 43), (275, 3, 51), (276, 3, 59),
        (277, 4, 67), (278, 4, 83), (279, 4, 99), (280, 4, 115),
        (281, 5, 131), (282, 5, 163), (283, 5, 195), (284, 5, 227),
        (285, 0, 258),
    ]
}

/// DEFLATE distance symbol table (RFC 1951 §3.2.5).
fn distance_table() -> &'static [(u8, u8, u16)] {
    &[
        (0, 0, 1), (1, 0, 2), (2, 0, 3), (3, 0, 4),
        (4, 1, 5), (5, 1, 7), (6, 2, 9), (7, 2, 13),
        (8, 3, 17), (9, 3, 25), (10, 4, 33), (11, 4, 49),
        (12, 5, 65), (13, 5, 97), (14, 6, 129), (15, 6, 193),
        (16, 7, 257), (17, 7, 385), (18, 8, 513), (19, 8, 769),
        (20, 9, 1025), (21, 9, 1537), (22, 10, 2049), (23, 10, 3073),
        (24, 11, 4097), (25, 11, 6145), (26, 12, 8193), (27, 12, 12289),
        (28, 13, 16385), (29, 13, 24577),
    ]
}

fn length_to_symbol(length: u16) -> usize {
    let table = length_table();
    for (i, (sym, _, base)) in table.iter().enumerate() {
        let next_base = if i + 1 < table.len() {
            table[i + 1].2
        } else {
            u16::MAX
        };
        if length >= *base && length < next_base {
            return usize::from(*sym);
        }
    }
    285
}

fn length_to_symbol_full(length: u16) -> (usize, u32, u16) {
    let table = length_table();
    for (sym, extra, base) in table.iter() {
        let next_base = if usize::from(*sym) - 257 + 1 < table.len() {
            table[usize::from(*sym) - 257 + 1].2
        } else {
            u16::MAX
        };
        if length >= *base && length < next_base {
            return (usize::from(*sym), u32::from(*extra), length - base);
        }
    }
    (285, 0, 0)
}

fn distance_to_symbol(distance: u16) -> usize {
    let table = distance_table();
    for (i, (sym, _, base)) in table.iter().enumerate() {
        let next_base = if i + 1 < table.len() {
            table[i + 1].2
        } else {
            u16::MAX
        };
        if distance >= *base && distance < next_base {
            return usize::from(*sym);
        }
    }
    29
}

fn distance_to_symbol_full(distance: u16) -> (usize, u32, u16) {
    let table = distance_table();
    for (sym, extra, base) in table.iter() {
        let next_base = if usize::from(*sym) + 1 < table.len() {
            table[usize::from(*sym) + 1].2
        } else {
            u16::MAX
        };
        if distance >= *base && distance < next_base {
            return (usize::from(*sym), u32::from(*extra), distance - base);
        }
    }
    (29, 0, 0)
}

/// LSB-first bit writer. Huffman codes are emitted MSB-first (reversed)
/// per RFC 1951.
struct BitWriter {
    out: Vec<u8>,
    bit_buf: u64,
    bit_count: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            bit_buf: 0,
            bit_count: 0,
        }
    }

    fn write_bits(&mut self, value: u64, nbits: u32) {
        if nbits == 0 {
            return;
        }
        self.bit_buf |= (value & ((1u64 << nbits) - 1).max(1)) << self.bit_count;
        self.bit_count += nbits;
        while self.bit_count >= 8 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf >>= 8;
            self.bit_count -= 8;
        }
    }

    fn write_huffman_code(&mut self, code: u16, len: u8) {
        if len == 0 {
            return;
        }
        // Reverse bits: Huffman codes are MSB-first, but DEFLATE
        // bitstream is LSB-first within each byte.
        let reversed = reverse_bits(u32::from(code), len) as u64;
        self.write_bits(reversed, u32::from(len));
    }

    fn flush_byte_aligned(&mut self) {
        if self.bit_count > 0 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf = 0;
            self.bit_count = 0;
        }
    }

    fn finish(self) -> Vec<u8> {
        self.out
    }
}

fn reverse_bits(mut v: u32, nbits: u8) -> u32 {
    let mut r = 0u32;
    for _ in 0..nbits {
        r = (r << 1) | (v & 1);
        v >>= 1;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_huffman_round_trips_simple_input() {
        let input = b"hello world hello world hello world hello world";
        let out = deflate_dynamic_huffman(input).expect("encode");
        assert!(out.is_some(), "should produce dynamic-Huffman block");
        let bytes = out.unwrap();
        // BFINAL=1, BTYPE=2 → first 3 bits = 0b101 = 5. Low 3 bits of
        // first byte: 0b00000101 = 0x05.
        assert_eq!(bytes[0] & 0x07, 0b101);
    }

    #[test]
    fn dynamic_huffman_handles_highly_repetitive() {
        let input: Vec<u8> = vec![b'a'; 4096];
        let out = deflate_dynamic_huffman(&input).expect("encode");
        assert!(out.is_some());
    }

    #[test]
    fn canonical_codes_satisfy_prefix_property() {
        let freqs = [10u32, 5, 8, 1, 1, 0, 0, 0];
        let mut lengths = [0u8; 8];
        build_huffman_lengths(&freqs, 15, &mut lengths);
        let codes = canonical_codes(&lengths, 15);
        // Verify no code is a prefix of another.
        for i in 0..8 {
            for j in (i + 1)..8 {
                if lengths[i] == 0 || lengths[j] == 0 {
                    continue;
                }
                let li = lengths[i] as u32;
                let lj = lengths[j] as u32;
                let ci = codes[i] as u32;
                let cj = codes[j] as u32;
                let min_len = li.min(lj);
                if (ci >> (li - min_len)) == (cj >> (lj - min_len)) {
                    panic!("prefix conflict: {ci} ({li}b) and {cj} ({lj}b)");
                }
            }
        }
    }

    #[test]
    fn run_length_encode_handles_zero_runs() {
        // 15 zeros triggers symbol 18 (handles 11-138 zero runs).
        let lengths: Vec<u8> = vec![0; 15].into_iter().chain([5, 5, 5, 5, 0]).collect();
        let syms = run_length_encode(&lengths);
        let has_18 = syms.iter().any(|s| s.symbol == 18);
        let has_16 = syms.iter().any(|s| s.symbol == 16);
        assert!(has_18, "long zero run should use symbol 18");
        assert!(has_16, "4-repeat should use symbol 16");
    }

    #[test]
    fn length_to_symbol_table_covers_3_to_258() {
        assert_eq!(length_to_symbol(3), 257);
        assert_eq!(length_to_symbol(10), 264);
        assert_eq!(length_to_symbol(258), 285);
    }
}
