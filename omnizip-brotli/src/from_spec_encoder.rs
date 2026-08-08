//! From-spec Brotli encoder (RFC 7932).
//!
//! Implements a complete Brotli encoder from scratch — no vendored
//! code from the upstream brotli crate. Produces Huffman-coded
//! metablocks with LZ77 matches and (optionally) static dictionary
//! references.
//!
//! ## Algorithm
//!
//! 1. **Match finding**: Hash-chain LZ77 (via `omnizip_codecs::matchfinder`)
//!    + brotli static dictionary lookup.
//! 2. **Parsing**: Greedy — take the longest match at each position.
//! 3. **Entropy coding**: Per-metablock Huffman codes built via
//!    length-limited package-merge (max 15 bits).
//! 4. **Framing**: Single ISLAST=1 metablock with NBLTYPES=1 /
//!    NTREES=1 / NPOSTFIX=0 / NDIRECT=0.
//!
//! ## Wire format
//!
//! All bits are written LSB-first per RFC 7932 §1. Canonical Huffman
//! codes (MSB-first by convention) are bit-reversed before emission
//! so the decoder's LSB-first reader produces the correct lookup index.
//!
//! ## Determinism
//!
//! All algorithms are deterministic. Same input → same output, always.

#![forbid(unsafe_code)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::needless_range_loop,
    clippy::collapsible_else_if,
    clippy::items_after_statements
)]

use crate::dictionary::find_dictionary_match;
use crate::prefix::kCmdLut;

/// Brotli window bits for the encoder (22 = 4 MB window).
const WINDOW_BITS: u8 = 22;

/// Window gap per RFC 7932 §9.1: 32 KiB reserved to disambiguate
/// dictionary references from LZ77 back-references.
const WINDOW_GAP: u32 = 0x8000;

/// Maximum backward distance for LZ77 matches.
/// Per RFC 7932 §9.1: max_backward_distance = (1 << WBITS) - WINDOW_GAP.
const MAX_BACKWARD_DISTANCE: u32 = (1 << WINDOW_BITS) - WINDOW_GAP;

/// Minimum match length for LZ77.
const MIN_MATCH: u32 = 4;

/// Maximum copy length per command (RFC 7932 §5).
const MAX_COPY: u32 = 273 - 2;

/// Number of short distance codes (RFC 7932 §10.4).
const NUM_SHORT: u32 = 16;

/// A parsed LZ77 command: insert `insert_len` literals, then copy
/// `copy_len` bytes from `distance` (1-based backward offset).
#[derive(Clone, Copy, Debug)]
struct Command {
    insert_len: u32,
    copy_len: u32,
    distance: u32,
}

/// LSB-first bit writer (Brotli's wire bit order per RFC 7932 §1).
struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            acc: 0,
            nbits: 0,
        }
    }

    fn write_bits(&mut self, value: u32, n: u32) {
        if n == 0 {
            return;
        }
        debug_assert!(n <= 32);
        let mask: u64 = if n >= 32 {
            u32::MAX as u64
        } else {
            (1u64 << n) - 1
        };
        self.acc |= (u64::from(value) & mask) << self.nbits;
        self.nbits += n;
        while self.nbits >= 8 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc >>= 8;
            self.nbits -= 8;
        }
    }

    fn byte_align(&mut self) {
        while self.nbits % 8 != 0 {
            self.write_bits(0, 1);
        }
    }

    fn flush(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc = 0;
            self.nbits = 0;
        }
        self.out
    }
}

/// Encode the WBITS field for `WINDOW_BITS` (RFC 7932 §9.1).
fn write_wbits(bw: &mut BitWriter) {
    bw.write_bits(1, 1);
    bw.write_bits(u32::from(WINDOW_BITS - 17), 3);
}

/// Reverse the low `n` bits of `v`. Used to convert MSB-first
/// canonical Huffman codes into the LSB-first wire representation.
fn reverse_bits(mut v: u32, n: u8) -> u32 {
    let mut r = 0u32;
    for _ in 0..n {
        r = (r << 1) | (v & 1);
        v >>= 1;
    }
    r
}

/// Compress input into a valid Brotli frame at quality 11 (maximum effort).
///
/// Equivalent to [`compress_with_quality`](fn.compress_with_quality.html) with q=11.
#[must_use]
pub fn compress(input: &[u8]) -> Vec<u8> {
    compress_with_quality(input, 11)
}

/// Compress at a given Brotli quality level (0–11).
///
/// Higher quality trades speed for ratio by:
/// - Increasing hash-chain depth (`max_chain_length`)
/// - Increasing the "good enough" match length (`nice_match`)
/// - Enabling static-dictionary lookups (q ≥ 2)
///
/// All levels produce RFC 7932-conformant Brotli streams decodable by
/// any standard Brotli decoder.
#[must_use]
pub fn compress_with_quality(input: &[u8], quality: i32) -> Vec<u8> {
    let q = quality.clamp(0, 11);
    if input.is_empty() {
        return empty_frame();
    }

    // Inputs ≤ 64 KiB: single metablock, Huffman or uncompressed.
    if input.len() < (1 << 16) {
        let uncompressed = encode_uncompressed_frame(input);
        let huffman = encode_huffman_frame_q(input, q);
        if !huffman.is_empty() && huffman.len() < uncompressed.len() {
            return huffman;
        }
        return uncompressed;
    }

    // Large inputs (> 64 KiB): split into 64 KiB-1 chunks and emit
    // each as a Huffman-coded metablock. Each chunk is independently
    // Huffman-coded with its own LZ77 + dictionary pass; the decoder
    // threads the cumulative output position so dictionary references
    // resolve at the correct global offset.
    let chunk_size = (1 << 16) - 1;
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);

    let mut offset = 0usize;
    while offset < input.len() {
        let end = (offset + chunk_size).min(input.len());
        let is_last = end == input.len();
        encode_huffman_chunk_into(&mut bw, &input[offset..end], offset, is_last, q);
        offset = end;
    }
    bw.flush()
}

/// Append all bits from `src` (bytes + accumulator) to `dst`.
#[allow(dead_code)]
fn append_writer(dst: &mut BitWriter, src: BitWriter) {
    for byte in src.out {
        dst.write_bits(u32::from(byte), 8);
    }
    if src.nbits > 0 {
        dst.write_bits(src.acc as u32, src.nbits);
    }
}

/// Encode one metablock (Huffman-coded) into the shared writer.
fn encode_huffman_chunk_into(
    bw: &mut BitWriter,
    input: &[u8],
    mlen_offset: usize,
    is_last: bool,
    quality: i32,
) {
    bw.write_bits(if is_last { 1 } else { 0 }, 1); // ISLAST
                                                   // ISLASTEMPTY only present when ISLAST=1; we never emit empty
                                                   // metablocks, so always 0 when present.
    if is_last {
        bw.write_bits(0, 1); // ISLASTEMPTY = 0
    }
    bw.write_bits(0, 2); // MNIBBLES = 0 (= 4 nibbles)
    let mlen_minus_1 = (input.len() - 1) as u32;
    for i in 0..4u32 {
        bw.write_bits((mlen_minus_1 >> (4 * i)) & 0xF, 4);
    }
    bw.write_bits(0, 1); // ISUNCOMPRESSED = 0

    bw.write_bits(0, 1); // NBLTYPESL = 1
    bw.write_bits(0, 1); // NBLTYPESI = 1
    bw.write_bits(0, 1); // NBLTYPESD = 1

    bw.write_bits(0, 2); // NPOSTFIX = 0
    bw.write_bits(0, 4); // NDMOEM = 0

    bw.write_bits(0, 2); // CONTEXT_MODE = LSB6
    bw.write_bits(0, 1); // NTREESL = 1
    bw.write_bits(0, 1); // NTREESD = 1

    let commands = parse_input_with_offset(input, mlen_offset, quality);
    let Some(stream) = build_symbol_stream(&commands, input) else {
        return;
    };

    let mut lit_freq = vec![0u32; 256];
    let mut cmd_freq = vec![0u32; 704];
    let mut dist_freq = vec![0u32; 64];
    for &b in &stream.literals {
        lit_freq[b as usize] += 1;
    }
    for &sym in &stream.cmd_symbols {
        cmd_freq[sym] += 1;
    }
    for &sym in &stream.dist_symbols {
        dist_freq[sym as usize] += 1;
    }

    let lit_lengths = omnizip_codecs::HuffmanLengths::build(&lit_freq, 15);
    let cmd_lengths = omnizip_codecs::HuffmanLengths::build(&cmd_freq, 15);
    let dist_lengths = omnizip_codecs::HuffmanLengths::build(&dist_freq, 15);

    let lit_codes = canonical_with_reverse(&lit_lengths);
    let cmd_codes = canonical_with_reverse(&cmd_lengths);
    let dist_codes = canonical_with_reverse(&dist_lengths);

    write_huffman_table(bw, &lit_lengths, 256);
    write_huffman_table(bw, &cmd_lengths, 704);
    write_huffman_table(bw, &dist_lengths, 64);

    let mut lit_iter = stream.literals.iter();
    let mut dist_iter = stream.dist_symbols.iter().zip(stream.dist_extras.iter());
    for (&cmd_sym, cmd) in stream.cmd_symbols.iter().zip(commands.iter()) {
        let (code, len) = cmd_codes[cmd_sym];
        bw.write_bits(code, u32::from(len));

        let entry = &kCmdLut[cmd_sym];
        if entry.insert_len_extra_bits > 0 {
            let extra = cmd.insert_len - u32::from(entry.insert_len_offset);
            bw.write_bits(extra, u32::from(entry.insert_len_extra_bits));
        }
        if entry.copy_len_extra_bits > 0 {
            let extra = cmd.copy_len - u32::from(entry.copy_len_offset);
            bw.write_bits(extra, u32::from(entry.copy_len_extra_bits));
        }

        for _ in 0..cmd.insert_len {
            let &b = lit_iter.next().expect("literal stream exhausted");
            let (lc, ll) = lit_codes[b as usize];
            bw.write_bits(lc, u32::from(ll));
        }

        if cmd.copy_len > 0 {
            let (&d_sym, &d_extra) = dist_iter.next().expect("distance stream exhausted");
            let (dc, dl) = dist_codes[d_sym as usize];
            bw.write_bits(dc, u32::from(dl));
            let nbits = distance_extra_bits(d_sym);
            if nbits > 0 {
                bw.write_bits(d_extra, nbits);
            }
        }
    }
}

/// Encode one metablock (uncompressed) into the shared writer.
/// Kept for the `multi_metablock_uncompressed_round_trips` test.
#[cfg(test)]
fn encode_uncompressed_chunk_into(bw: &mut BitWriter, input: &[u8], is_last: bool) {
    bw.write_bits(if is_last { 1 } else { 0 }, 1); // ISLAST
    if is_last {
        bw.write_bits(0, 1); // ISLASTEMPTY = 0
    }
    bw.write_bits(0, 2); // MNIBBLES = 0 (= 4 nibbles)
    let mlen_minus_1 = (input.len() - 1) as u64;
    for i in 0..4u32 {
        bw.write_bits(((mlen_minus_1 >> (4 * u64::from(i))) & 0xF) as u32, 4);
    }
    bw.write_bits(1, 1); // ISUNCOMPRESSED = 1
    bw.byte_align();
    for &b in input {
        bw.write_bits(u32::from(b), 8);
    }
}

// ---------------------------------------------------------------------------
// Uncompressed metablock (RFC 7932 §9.2)
// ---------------------------------------------------------------------------

fn encode_uncompressed_frame(input: &[u8]) -> Vec<u8> {
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);

    bw.write_bits(1, 1); // ISLAST = 1
    bw.write_bits(0, 1); // ISLASTEMPTY = 0

    let mnibbles_field: u32 = if input.len() < (1 << 16) { 0 } else { 2 };
    bw.write_bits(mnibbles_field, 2);

    let nibbles: u32 = if mnibbles_field == 0 {
        4
    } else {
        mnibbles_field + 3
    };
    let mlen_minus_1 = (input.len() - 1) as u64;
    for i in 0..nibbles {
        let nib = ((mlen_minus_1 >> (4 * u64::from(i))) & 0xF) as u32;
        bw.write_bits(nib, 4);
    }

    bw.write_bits(1, 1); // ISUNCOMPRESSED = 1
    bw.byte_align();

    let mut out = bw.flush();
    out.extend_from_slice(input);
    out
}

fn empty_frame() -> Vec<u8> {
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);
    bw.write_bits(1, 1); // ISLAST = 1
    bw.write_bits(1, 1); // ISLASTEMPTY = 1
    bw.flush()
}

// ---------------------------------------------------------------------------
// Huffman-coded metablock
// ---------------------------------------------------------------------------

/// Encode the entire input as a single Huffman-coded metablock (fallback
/// for inputs ≤ 64 KiB). Calls the chunk encoder with mlen_offset=0 and
/// is_last=true, then prepends WBITS.
fn encode_huffman_frame_q(input: &[u8], quality: i32) -> Vec<u8> {
    if input.is_empty() || input.len() >= (1 << 16) {
        return Vec::new();
    }
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);
    encode_huffman_chunk_into(&mut bw, input, 0, true, quality);
    bw.flush()
}

/// A parsed symbol stream ready for entropy coding.
struct SymbolStream {
    /// Literal bytes in insertion order.
    literals: Vec<u8>,
    /// Command symbols (indices into kCmdLut, 0..704).
    cmd_symbols: Vec<usize>,
    /// Distance symbols (0..63) — one per command with copy_len > 0.
    dist_symbols: Vec<u32>,
    /// Distance extra-bit values, parallel to `dist_symbols`.
    dist_extras: Vec<u32>,
}

/// Build the entropy-coded symbol stream from commands.
///
/// For each command:
/// - Look up the matching entry in `kCmdLut` (cell_idx ≥ 2 for explicit
///   distance; we never emit implicit-distance commands).
/// - Compute the distance symbol + extra bits via the long-code formula
///   (RFC 7932 §10.4).
fn build_symbol_stream(commands: &[Command], input: &[u8]) -> Option<SymbolStream> {
    let mut literals = Vec::new();
    let mut cmd_symbols = Vec::with_capacity(commands.len());
    let mut dist_symbols = Vec::new();
    let mut dist_extras = Vec::new();

    for cmd in commands {
        // Pull literals from the input by insert_len.
        // (The encoder guarantees insert_len + copy_len consumes the
        // input sequentially — we just trust the parser here.)
        let _ = input;

        let cmd_sym = find_cmd_symbol(cmd.insert_len, cmd.copy_len)?;
        cmd_symbols.push(cmd_sym);

        if cmd.copy_len > 0 {
            let (sym, extra) = encode_distance(cmd.distance);
            dist_symbols.push(sym);
            dist_extras.push(extra);
        }
    }

    // Extract literals in stream order from commands (re-derive from input
    // via a sequential cursor — the parser already grouped them).
    // For correctness we re-walk commands against a cursor.
    let mut cur = 0usize;
    for cmd in commands {
        let end = cur + cmd.insert_len as usize;
        // We don't have direct access to input here in this signature, but
        // literals were already pushed by parse_input via the commands'
        // insert ranges. Push them now from `input` to keep this function
        // total.
        literals.extend_from_slice(&input[cur..end]);
        cur = end + cmd.copy_len as usize;
    }

    Some(SymbolStream {
        literals,
        cmd_symbols,
        dist_symbols,
        dist_extras,
    })
}

/// Find the kCmdLut symbol matching (insert_len, copy_len).
///
/// For `copy_len > 0`: matches entries with `distance_code == -1`
/// (cell_idx ≥ 2) so an explicit distance code is read by the decoder.
///
/// For `copy_len == 0` (insert-only trailing command): matches any
/// entry whose `insert_len_offset` is in range and whose
/// `copy_len_offset == 2` (smallest). The decoder short-circuits at
/// metablock end without executing the copy, so the phantom copy_len
/// is harmless.
fn find_cmd_symbol(insert_len: u32, copy_len: u32) -> Option<usize> {
    let phantom = copy_len == 0;
    let effective_copy = if phantom { 2 } else { copy_len };
    for (i, entry) in kCmdLut.iter().enumerate() {
        if !phantom && entry.distance_code >= 0 {
            continue;
        }
        let ins_lo = u32::from(entry.insert_len_offset);
        let ins_hi = ins_lo + ((1u32) << u32::from(entry.insert_len_extra_bits)) - 1;
        let cpy_lo = u32::from(entry.copy_len_offset);
        let cpy_hi = cpy_lo + ((1u32) << u32::from(entry.copy_len_extra_bits)) - 1;
        if (ins_lo..=ins_hi).contains(&insert_len) && (cpy_lo..=cpy_hi).contains(&effective_copy) {
            return Some(i);
        }
    }
    None
}

/// Encode an LZ77 distance as a (symbol, extra_bits) pair.
///
/// Uses only long codes (symbol ≥ NUM_SHORT=16) since short codes 0-15
/// reference the recent-distances ring buffer, which would require
/// stateful tracking. Long codes are stateless and correct at the cost
/// of slightly larger output.
///
/// Long-code formula (inverted from RFC 7932 §10.4):
///   distval = sym - 16
///   nbits = (distval >> 1) + 1
///   offset = ((2 + (distval & 1)) << nbits) - 4
///   distance = offset + extra + 1
fn encode_distance(distance: u32) -> (u32, u32) {
    let d = distance.saturating_sub(1);
    // Smallest nbits such that the distance fits in a (2 or 3) << nbits bucket.
    // Find smallest nbits where d < (3 << (nbits+1)) - 3.
    let mut nbits: u32 = 1;
    while nbits < 24 {
        let limit_even = (4u32 << (nbits - 1)).saturating_sub(4) + (1u32 << nbits);
        let limit_odd = (6u32 << (nbits - 1)).saturating_sub(4) + (1u32 << nbits);
        let limit = limit_even.max(limit_odd);
        if d < limit {
            break;
        }
        nbits += 1;
    }
    // Decide even/odd bucket based on which contains d.
    let even_offset = (4u32 << (nbits - 1)).saturating_sub(4);
    let odd_offset = (6u32 << (nbits - 1)).saturating_sub(4);
    let (postfix_bit, base) = if d >= odd_offset {
        (1, odd_offset)
    } else {
        (0, even_offset)
    };
    let distval = (nbits - 1) * 2 + postfix_bit;
    let sym = NUM_SHORT + distval;
    let extra = d - base;
    (sym, extra)
}

/// Number of extra bits for a distance symbol.
fn distance_extra_bits(sym: u32) -> u32 {
    if sym < NUM_SHORT {
        return 0;
    }
    let distval = sym - NUM_SHORT;
    (distval >> 1) + 1
}

/// Parse input into commands using LZ77 + static dictionary.
///
/// Tracks the output position to compute `max_distance = min(pos,
/// max_backward_distance)` per RFC 7932 §10.4. Dictionary references
/// use `distance = max_distance + 1 + address` so the decoder resolves
/// them via the static dictionary lookup path.
/// Parse input into commands using LZ77 + static dictionary.
///
/// Convenience wrapper for chunks at the start of the input (offset=0).
#[allow(dead_code)]
fn parse_input(input: &[u8]) -> Vec<Command> {
    parse_input_with_offset(input, 0, 11)
}

/// Parse input into commands using LZ77 + static dictionary, with a
/// non-zero `mlen_offset` for chunks in a multi-metablock frame.
///
/// `mlen_offset` is added to local `pos` when computing per-position
/// `max_distance`, so dictionary references use the same distance
/// formula the decoder will use.
///
/// `quality` (0–11) controls match-finder effort and whether the
/// static dictionary is consulted.
fn parse_input_with_offset(input: &[u8], mlen_offset: usize, quality: i32) -> Vec<Command> {
    let n = input.len();
    let mut commands = Vec::new();

    // Quality → match-finder effort. Brotli's reference encoder scales
    // hash-chain depth and nice_match roughly exponentially with q; we
    // use a simpler piecewise mapping that captures the main tiers.
    let (max_chain, nice_match, use_dict) = match quality {
        0..=1 => (4, 8, false),
        2..=3 => (16, 16, true),
        4..=5 => (32, 32, true),
        6..=7 => (64, 64, true),
        8..=9 => (128, 128, true),
        _ => (256, 271, true), // q 10–11
    };

    let config = omnizip_codecs::HashChainConfig {
        dict_size: MAX_BACKWARD_DISTANCE,
        min_match: MIN_MATCH,
        max_chain_length: max_chain,
        nice_match,
        hash_log: 16,
    };
    let mut mf = omnizip_codecs::HashChainMatchFinder::new(input, config);

    let mut pos = 0usize;
    let mut insert_start = 0usize;
    while pos < n {
        // Global output position (across metablocks) for max_distance.
        let global_pos = mlen_offset + pos;
        let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);

        let lz77 = if pos + MIN_MATCH as usize <= n {
            mf.advance();
            mf.find_match(pos)
        } else {
            None
        };

        let lz77_valid = lz77.as_ref().map_or(false, |m| m.distance <= max_dist);

        let best = if lz77_valid {
            let m = lz77.as_ref().unwrap();
            if m.length >= 8 || !use_dict {
                Some((m.distance, m.length, false))
            } else {
                let dict = find_dictionary_match(input, pos, max_dist);
                match dict {
                    Some((d, l)) if l > m.length => Some((d, l, true)),
                    _ => Some((m.distance, m.length, false)),
                }
            }
        } else if use_dict {
            let dict = find_dictionary_match(input, pos, max_dist);
            dict.map(|(d, l)| (d, l, true))
        } else {
            None
        };

        if let Some((distance, length, _is_dict)) = best {
            if length >= MIN_MATCH && distance > 0 {
                let copy_len = length.min(MAX_COPY).max(MIN_MATCH);
                let insert_len = (pos - insert_start) as u32;
                commands.push(Command {
                    insert_len,
                    copy_len,
                    distance,
                });
                let advance = copy_len as usize;
                for _ in 1..advance {
                    if pos + 1 < n {
                        pos += 1;
                        mf.advance();
                    }
                }
                pos += 1;
                insert_start = pos;
                continue;
            }
        }
        pos += 1;
    }

    // Trailing literals: emit a separate trailing-insert command (with
    // copy_len=0, encoded as a phantom copy_len=2 that the decoder
    // short-circuits past at metablock end).
    if insert_start < n {
        let trailing = (n - insert_start) as u32;
        commands.push(Command {
            insert_len: trailing,
            copy_len: 0,
            distance: 0,
        });
    }

    commands
}

/// Build canonical Huffman codes (MSB-first) and bit-reverse each code
/// to its LSB-first wire form. Returns `Vec<(wire_code, length)>`.
///
/// Special case: when the alphabet has exactly one non-zero symbol
/// (the simple-form NSYM=1 layout), the decoder reads 0 bits per
/// occurrence via its `single_symbol` fast path. We reflect that here
/// by returning (0, 0) for the sole non-zero symbol so the writer
/// emits nothing for it.
fn canonical_with_reverse(lengths: &omnizip_codecs::HuffmanLengths) -> Vec<(u32, u8)> {
    let nonzero_count = lengths.lengths.iter().filter(|&&l| l > 0).count();
    let codes = lengths.canonical_codes();
    codes
        .into_iter()
        .map(|(c, l)| {
            let l = if nonzero_count == 1 { 0 } else { l };
            (reverse_bits(c, l), l)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Huffman table encoding (RFC 7932 §9.5)
// ---------------------------------------------------------------------------

/// Code-length code prefix: maps each code-length value (0-5) to its
/// (wire_value, bits) encoding via the fixed K_CL_PREFIX code.
///
/// Derived from the decoder's K_CL_PREFIX_VALUE / K_CL_PREFIX_LENGTH
/// tables (decoder.rs:645-646). Each entry is the LSB-first stream
/// representation of the prefix code for that value.
const CL_CODE_TO_WIRE: [(u32, u8); 6] = [
    (0b00, 2),   // value 0
    (0b0111, 4), // value 1
    (0b011, 3),  // value 2
    (0b10, 2),   // value 3
    (0b01, 2),   // value 4
    (0b1111, 4), // value 5
];

/// CODE_LENGTH_CODE_ORDER per RFC 7932 §9.5.2.
const CODE_LENGTH_CODE_ORDER: [u8; 18] =
    [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// Write a Huffman table (RFC 7932 §9.5).
///
/// Uses the complex form (HSKIP=0) for any alphabet size. The
/// implementation does NOT emit RLE symbols (16/17), which is slightly
/// wasteful for sparse code-length arrays but produces correct output.
///
/// The number of code-length entries written matches what the decoder
/// will read: the decoder breaks its read loop once the code-length
/// prefix code's "space" is fully consumed (sum of 32>>len = 32). We
/// replicate that break here so the bit position after this table
/// matches the decoder's expectation.
fn write_huffman_table(
    bw: &mut BitWriter,
    lengths: &omnizip_codecs::HuffmanLengths,
    alphabet: usize,
) {
    // Special case: 0 or 1 non-zero symbols both use the simple form
    // (HSKIP=1, NSYM=1). The complex form requires a valid prefix code
    // over the code-length alphabet, which doesn't exist when no main
    // alphabet symbols are used (e.g. literal table for a metablock
    // with zero literals).
    let nonzero: Vec<usize> = lengths
        .lengths
        .iter()
        .enumerate()
        .filter(|(_, &l)| l > 0)
        .map(|(i, _)| i)
        .collect();
    if nonzero.len() <= 1 {
        let sym = nonzero.first().copied().unwrap_or(0);
        write_simple_one_symbol(bw, alphabet, sym);
        return;
    }

    // Complex form: HSKIP = 0.
    bw.write_bits(0, 2);

    // Build the RLE-compressed code-length sequence (RFC 7932 §9.5).
    // Symbol 16 = repeat previous code length 3+extra(2) times (3-6).
    // Symbol 17 = repeat zero 3+extra(3) times (3-10).
    // This reduces table description size for inputs with many
    // same-length codes (typical for near-uniform byte distributions).
    // Returns Vec<(symbol, extra_bits)> so the writer doesn't need to
    // re-derive counts from the raw lengths.
    let rle = build_rle_sequence(&lengths.lengths[..alphabet]);

    // Build a sub-Huffman over the 18-symbol code-length alphabet,
    // using frequencies from the RLE-compressed sequence (not the raw
    // lengths). This accounts for symbols 16/17 in the code-length code.
    let mut cl_freq = [0u32; 18];
    for &(sym, _) in &rle {
        cl_freq[usize::from(sym)] += 1;
    }
    let cl_lengths = omnizip_codecs::HuffmanLengths::build(&cl_freq, 5);
    let cl_codes = cl_lengths.canonical_codes();

    // When the code-length code has exactly one non-zero symbol, the
    // decoder's single_symbol fast path reads 0 bits per occurrence.
    // We detect this and write 0 bits below.
    let cl_single = cl_lengths.lengths.iter().filter(|&&l| l > 0).count() == 1;

    // Walk CODE_LENGTH_CODE_ORDER, emitting each code-length value via
    // the fixed K_CL_PREFIX code. Stop early once the code-length prefix
    // code's space is fully consumed (mirrors the decoder's break).
    let mut space: u32 = 32;
    let mut num_codes: u32 = 0;
    for &sym in &CODE_LENGTH_CODE_ORDER {
        let len = cl_lengths.lengths[usize::from(sym)];
        let (wire, nbits) = CL_CODE_TO_WIRE[usize::from(len)];
        bw.write_bits(wire, u32::from(nbits));

        if len != 0 {
            space = space.wrapping_sub(32u32 >> u32::from(len));
            num_codes += 1;
            // Decoder breaks when space.wrapping_sub(1) >= 32, i.e. when
            // space has reached 0 (or underflowed, which can't happen
            // for a valid prefix code).
            if num_codes != 1 && space.wrapping_sub(1) >= 32 {
                break;
            }
        }
    }

    // Write the actual code lengths using the code-length Huffman code,
    // emitting RLE symbols (16/17) from the pre-computed sequence.
    // The decoder exits its read loop once the main prefix code's space
    // is fully consumed (sum of 32768>>len = 32768). We replicate that
    // break here so the bit position after this table matches.
    let mut main_space: u32 = 32768;
    let mut prev_code_len: u8 = 8;
    for &(sym, extra) in &rle {
        let (val, count) = match sym {
            16 => (prev_code_len, 3 + extra as usize),
            17 => (0u8, 3 + extra as usize),
            v => (v, 1usize),
        };

        if !cl_single {
            let (code, clen) = cl_codes[usize::from(sym)];
            let wire = reverse_bits(code, clen);
            bw.write_bits(wire, u32::from(clen));
            if sym == 16 {
                bw.write_bits(extra as u32, 2);
            } else if sym == 17 {
                bw.write_bits(extra as u32, 3);
            }
        }

        if val != 0 {
            prev_code_len = val;
            for _ in 0..count {
                main_space = main_space.wrapping_sub(32768u32 >> u32::from(val));
                if main_space == 0 {
                    return;
                }
            }
        }
    }
}

/// Build an RLE-compressed code-length sequence from raw lengths.
///
/// Returns `Vec<(symbol, extra_bits)>`:
/// - Symbols 0-15: literal code-length values (extra = 0).
/// - Symbol 16: repeat previous code length `3 + extra` times (extra ∈ 0..4).
/// - Symbol 17: repeat zero `3 + extra` times (extra ∈ 0..8).
///
/// To avoid the decoder's iterated accumulator (which combines
/// consecutive repeat symbols non-linearly), a literal symbol is
/// inserted between consecutive repeats of the same value.
fn build_rle_sequence(lengths: &[u8]) -> Vec<(u8, u8)> {
    let n = lengths.len();
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let val = lengths[i];
        let mut run = 1usize;
        while i + run < n && lengths[i + run] == val {
            run += 1;
        }

        if val == 0 {
            // Emit leading 1-2 zeros individually (symbol 17 needs ≥3).
            let lead = run.min(2);
            for _ in 0..lead {
                out.push((0, 0));
            }
            run -= lead;
            i += lead;
            // Use symbol 17 for remaining zero runs (count 3-10).
            // Insert a literal 0 between consecutive symbol-17s to
            // avoid the decoder's iterated accumulator.
            while run >= 3 {
                let chunk = run.min(10);
                out.push((17, (chunk - 3) as u8));
                run -= chunk;
                i += chunk;
                if run >= 3 {
                    out.push((0, 0));
                    run -= 1;
                    i += 1;
                }
            }
            for _ in 0..run {
                out.push((0, 0));
            }
            i += run;
        } else {
            out.push((val, 0));
            i += 1;
            run -= 1;
            // Use symbol 16 for repeat runs (count 3-6).
            // Insert a literal between consecutive symbol-16s.
            while run >= 3 {
                let chunk = run.min(6);
                out.push((16, (chunk - 3) as u8));
                run -= chunk;
                i += chunk;
                if run >= 3 {
                    out.push((val, 0));
                    run -= 1;
                    i += 1;
                }
            }
            for _ in 0..run {
                out.push((val, 0));
            }
            i += run;
        }
    }
    out
}

/// Write a single-symbol Huffman table in simple form (RFC 7932 §9.5.1).
fn write_simple_one_symbol(bw: &mut BitWriter, alphabet: usize, sym: usize) {
    bw.write_bits(0b01, 2); // HSKIP = 1
    bw.write_bits(0b00, 2); // NSYM = 1 (encoded as 0b00)
    let bits_per_sym = ceil_log2(alphabet as u32);
    bw.write_bits(sym as u32, bits_per_sym);
}

/// ⌈log2(n)⌉ for n ≥ 1.
fn ceil_log2(n: u32) -> u32 {
    if n <= 1 {
        return 0;
    }
    32 - (n - 1).leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder;

    #[test]
    fn empty_round_trips() {
        let compressed = compress(&[]);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert!(decoded.is_empty());
    }

    #[test]
    fn short_round_trips() {
        let input = b"hello world";
        let compressed = compress(input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded.as_slice(), input.as_ref());
    }

    #[test]
    fn repetitive_round_trips() {
        let input = b"abcabcabcabc".repeat(10);
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn binary_round_trips() {
        let input: Vec<u8> = (0..256u32).map(|i| (i % 256) as u8).collect();
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn csv_round_trips() {
        let input: Vec<u8> = (0..100)
            .map(|i| format!("row_{},{},value_{}\n", i, i * 2, i % 7))
            .collect::<String>()
            .into_bytes();
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn determinism() {
        let input = b"determinism test input with repetition repetition";
        let a = compress(input);
        let b = compress(input);
        assert_eq!(a, b);
    }

    #[test]
    fn compresses_repetitive_input() {
        // CSV-like input should compress to less than half its size when
        // the Huffman path lands. If the uncompressed fallback wins,
        // the test still passes (we just verify correctness above).
        let input: Vec<u8> = b"abcabcabcabc".repeat(50);
        let compressed = compress(&input);
        assert!(
            compressed.len() < input.len(),
            "compressed {} should be < input {}",
            compressed.len(),
            input.len()
        );
    }

    #[test]
    fn compresses_text_input() {
        let input: Vec<u8> = b"The quick brown fox jumps over the lazy dog. ".repeat(50);
        let compressed = compress(&input);
        assert!(
            compressed.len() < input.len(),
            "text compressed {} should be < input {}",
            compressed.len(),
            input.len()
        );
    }

    #[test]
    fn compresses_csv_input() {
        let input: Vec<u8> = (0..100)
            .map(|i| format!("row_{},{},value_{}\n", i, i * 2, i % 7))
            .collect::<String>()
            .into_bytes();
        let compressed = compress(&input);
        assert!(
            compressed.len() < input.len(),
            "csv compressed {} should be < input {}",
            compressed.len(),
            input.len()
        );
    }

    #[test]
    fn bit_writer_lsb_first() {
        let mut bw = BitWriter::new();
        bw.write_bits(1, 1);
        bw.write_bits(2, 2);
        let out = bw.flush();
        assert_eq!(out, vec![0b0101]);
    }

    #[test]
    fn large_repetitive_input_round_trips() {
        // 200 KiB. The encoder currently falls back to a single
        // uncompressed metablock for inputs >64 KiB (multi-metablock
        // Huffman path is TODO). Verify round-trip correctness.
        let input: Vec<u8> = b"the quick brown fox jumps over the lazy dog. ".repeat(5000);
        assert!(input.len() > 65_536);
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn near_max_metablock_round_trips() {
        // 60 KiB — fits in a single 4-nibble MLEN metablock.
        let input: Vec<u8> = vec![0u8; 60_000];
        let compressed = compress(&input);
        eprintln!("input {} -> compressed {}", input.len(), compressed.len());
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn random_single_metablock_huffman_round_trips() {
        // 4 KiB of pseudo-random data — exercises Huffman table
        // encoding with many distinct code lengths.
        let input: Vec<u8> = (0..4_000u32)
            .map(|i| (i.wrapping_mul(2654435761)) as u8)
            .collect();
        let compressed = compress(&input);
        eprintln!(
            "random 4KB: input {} -> compressed {}",
            input.len(),
            compressed.len()
        );
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn two_chunk_input_round_trips() {
        // Just barely larger than 64 KiB — exercises the uncompressed
        // fallback path (multi-metablock Huffman is TODO).
        let input: Vec<u8> = vec![0u8; 70_000];
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn two_chunk_random_input_round_trips() {
        let input: Vec<u8> = (0..70_000u32)
            .map(|i| (i.wrapping_mul(2654435761)) as u8)
            .collect();
        let compressed = compress(&input);
        eprintln!(
            "rand input {} -> compressed {}",
            input.len(),
            compressed.len()
        );
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    /// Manual bit-position check for multi-metablock (debug aid).
    #[test]
    fn multi_metablock_uncompressed_round_trips() {
        // Two raw uncompressed metablocks back-to-back. Both 100 bytes.
        let chunk1 = vec![0xAAu8; 100];
        let chunk2 = vec![0xBBu8; 100];
        let mut bw = BitWriter::new();
        write_wbits(&mut bw);
        encode_uncompressed_chunk_into(&mut bw, &chunk1, false);
        encode_uncompressed_chunk_into(&mut bw, &chunk2, true);
        let compressed = bw.flush();
        let decoded = decoder::decode(&compressed).expect("decode");
        let expected: Vec<u8> = chunk1.iter().chain(chunk2.iter()).copied().collect();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn multi_metablock_huffman_round_trips() {
        // Two Huffman-coded metablocks. Each is a small repetitive chunk
        // that compresses well.
        let chunk1 = b"abcabc".repeat(20);
        let chunk2 = b"xyzxyz".repeat(20);
        let mut bw = BitWriter::new();
        write_wbits(&mut bw);
        encode_huffman_chunk_into(&mut bw, &chunk1, 0, false, 11);
        encode_huffman_chunk_into(&mut bw, &chunk2, chunk1.len(), true, 11);
        let compressed = bw.flush();
        eprintln!("compressed: {} bytes", compressed.len());
        let decoded = decoder::decode(&compressed).expect("decode");
        let expected: Vec<u8> = chunk1.iter().chain(chunk2.iter()).copied().collect();
        assert_eq!(decoded, expected);
    }

    /// Probe: does the multi-metablock Huffman path actually work on
    /// large (>64 KiB) inputs? If this passes, the "bit-position bug"
    /// mentioned in the compress() comment is already fixed, and we
    /// can enable Huffman for large inputs.
    #[test]
    fn multi_metablock_huffman_large_input() {
        let input: Vec<u8> = b"the quick brown fox jumps over the lazy dog. ".repeat(5000);
        assert!(input.len() > 65_536);

        let chunk_size = (1 << 16) - 1;
        let mut bw = BitWriter::new();
        write_wbits(&mut bw);
        let mut offset = 0usize;
        while offset < input.len() {
            let end = (offset + chunk_size).min(input.len());
            let is_last = end == input.len();
            encode_huffman_chunk_into(&mut bw, &input[offset..end], offset, is_last, 11);
            offset = end;
        }
        let compressed = bw.flush();
        eprintln!(
            "large multi-mb huffman: input {} -> compressed {} ({:.1}%)",
            input.len(),
            compressed.len(),
            compressed.len() as f64 / input.len() as f64 * 100.0
        );
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn constant_256_round_trips() {
        let input = vec![0u8; 256];
        let compressed = compress(&input);
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
    }

    #[test]
    fn reverse_bits_works() {
        assert_eq!(reverse_bits(0b1010, 4), 0b0101);
        assert_eq!(reverse_bits(0b1, 1), 0b1);
        assert_eq!(reverse_bits(0b10, 2), 0b01);
    }

    #[test]
    fn distance_round_trips_in_decoder_formula() {
        // For a range of distances, encode_distance + the decoder formula
        // (decode_distance_from_code with npostfix=0, num_direct=16)
        // should reproduce the original distance.
        for d in [1u32, 2, 3, 4, 5, 10, 17, 100, 1000, 65_536, 1_000_000] {
            let (sym, extra) = encode_distance(d);
            // Decoder formula: distval = sym - 16, nbits = (distval >> 1) + 1,
            // offset = ((2 + (distval & 1)) << nbits) - 4,
            // distance = offset + extra + 1.
            let distval = i32::from(sym as i32 - 16);
            let nbits = ((distval as u32) >> 1) + 1;
            let offset = (((distval & 1) + 2) << nbits) - 4;
            let decoded = (offset + extra as i32 + 1) as u32;
            assert_eq!(
                decoded, d,
                "distance {} round-trip failed: sym={}, extra={}",
                d, sym, extra
            );
        }
    }

    #[test]
    fn wbits_decodes_to_22() {
        let frame = compress(b"abc");
        let (parsed, _) = decoder::parse_frame_header(&frame, 0).expect("parse header");
        assert_eq!(parsed.window_bits, WINDOW_BITS);
    }

    #[test]
    fn quality_distinguishes_output() {
        // Same input at q=0 vs q=11 should produce different output
        // (lower quality = lazier match finding = different commands).
        let input: Vec<u8> = b"the quick brown fox jumps over the lazy dog. ".repeat(50);
        let lo = compress_with_quality(&input, 0);
        let hi = compress_with_quality(&input, 11);
        assert_ne!(
            lo, hi,
            "q=0 and q=11 should produce different compressed output"
        );
        // Both must round-trip correctly.
        let dec_lo = decoder::decode(&lo).expect("decode q=0");
        let dec_hi = decoder::decode(&hi).expect("decode q=11");
        assert_eq!(dec_lo, input);
        assert_eq!(dec_hi, input);
        // Higher quality should compress at least as well as lower.
        // (They can be equal for trivially compressible input, but q=11
        // should never be *larger*.)
        assert!(
            hi.len() <= lo.len(),
            "q=11 ({}) should be <= q=0 ({})",
            hi.len(),
            lo.len()
        );
    }

    #[test]
    fn all_qualities_round_trip() {
        let inputs: Vec<Vec<u8>> = vec![
            b"hello world".to_vec(),
            b"abcabcabcabc".repeat(20),
            (0..1000u32)
                .map(|i| (i.wrapping_mul(2654435761)) as u8)
                .collect(),
            b"the quick brown fox jumps over the lazy dog. ".repeat(100),
        ];
        for q in 0..=11 {
            for input in &inputs {
                let compressed = compress_with_quality(input, q);
                let decoded = decoder::decode(&compressed)
                    .unwrap_or_else(|e| panic!("decode q={q} input {}b: {e}", input.len()));
                assert_eq!(decoded, *input, "round-trip q={q} input {}b", input.len());
            }
        }
    }
}
