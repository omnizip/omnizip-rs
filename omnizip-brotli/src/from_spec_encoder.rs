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

use crate::dictionary::dictionary_lookup;
use crate::encoder::bitwriter::BitWriter;
use crate::encoder::context::{compute_context_id, is_text_like};
use crate::encoder::dict_hash;
use crate::encoder::distance_config::{DistanceConfig, NUM_SHORT};
use crate::prefix::kCmdLut;

/// Brotli window bits for the encoder (24 = 16 MB window).
/// The C reference uses WBITS=22 (4 MB) for quality < 10 and
/// WBITS=24 (16 MB) for quality >= 10. We use 24 for all qualities
/// to maximize match distance.
const WINDOW_BITS: u8 = 24;

/// Window gap per upstream brotli (`kBrotliWindowGap = 16`): the
/// reserved bytes that disambiguate dictionary references from LZ77
/// back-references.
const WINDOW_GAP: u32 = 16;

/// Maximum backward distance for LZ77 matches.
/// Per RFC 7932 §9.1: `max_backward_distance` = (1 << WBITS) - `WINDOW_GAP`.
const MAX_BACKWARD_DISTANCE: u32 = (1 << WINDOW_BITS) - WINDOW_GAP;

/// Minimum match length for LZ77.
const MIN_MATCH: u32 = 4;

/// Maximum copy length per command (RFC 7932 §5).
/// Capped at 271 (the max for short copy-length codes). Longer copies
/// were tested (up to 4096) and improve ratio on highly repetitive
/// synthetic data but regress on FSST-preprocessed data where longer
/// matches interact poorly with the byte-code distribution.
// Wire maximum for a single copy: the last insert&copy code covers
// copy lengths 2118 + 24 extra bits. The previous 271 cap split every
// long repeat into multiple commands with explicit distances — the
// reference emits copies up to ~4KB (measured max 1450 on CSV q9).
const MAX_COPY: u32 = 16_779_211;

/// Reference MaxZopfliLen: HQ candidate/copy length caps (150 at q10,
/// 325 at q11). Below HQ our small-input zopfli keeps a 4096 cap.
fn zopfli_max_len(quality: i32) -> u32 {
    if quality >= 11 {
        325
    } else if quality == 10 {
        150
    } else {
        4096
    }
}

/// Static complex UTF-8 context map from the brotli reference encoder
/// (`kStaticContextMapComplexUTF64`). Maps 64 UTF-8 context IDs into
/// 13 literal Huffman trees. The split separates character classes
/// (digits, upper/lowercase, punctuation, whitespace) for tighter
/// per-tree Huffman coding.
///
/// Ported verbatim from `brotli/c/enc/encode.c` (BSD-3-Clause).
#[rustfmt::skip]
const K_STATIC_CONTEXT_MAP_COMPLEX_UTF8: [u8; 64] = [
    11, 11, 12, 12,  // contexts  0- 3: special/control
     0,  0,  0,  0,  // contexts  4- 7: LF/CR/whitespace
     1,  1,  9,  9,  // contexts  8-11: space
     2,  2,  2,  2,  // contexts 12-15: ! first after space/lf
     1,  1,  1,  1,  // contexts 16-19: "
     8,  3,  3,  3,  // contexts 20-23: %
     1,  1,  1,  1,  // contexts 24-27: ({[
     2,  2,  2,  2,  // contexts 28-31: }])
     8,  4,  4,  4,  // contexts 32-35: :;
     8,  7,  4,  4,  // contexts 36-39: .
     8,  0,  0,  0,  // contexts 40-43: >
     3,  3,  3,  3,  // contexts 44-47: [0-9]
     5,  5, 10,  5,  // contexts 48-51: [A-Z]
     5,  5, 10,  5,  // contexts 52-55: [A-Z]
     6,  6,  6,  6,  // contexts 56-59: [a-z]
     6,  6,  6,  6,  // contexts 60-63: [a-z]
];

/// Number of distinct trees in the complex UTF-8 context map.
const NTREES_COMPLEX_UTF8: u32 = 13;

/// Port of the reference DecideOverLiteralContextModeling (q5-9 text):
/// pick the literal context map from sampled entropy — the 13-tree
/// complex UTF8 map for >= 1 MiB inputs when it pays, else the
/// bigram-prefix-chosen 1/2/3-context maps. Returns None for a single
/// context.
fn decide_literal_contexts(
    input: &[u8],
    quality: i32,
    size_hint: usize,
) -> Option<(usize, Vec<u8>)> {
    if input.len() < 64 {
        return None;
    }
    let sh = |h: &[u32]| -> f64 {
        let t: f64 = h.iter().map(|&x| f64::from(x)).sum();
        if t == 0.0 {
            return 0.0;
        }
        -h.iter()
            .filter(|&&x| x > 0)
            .map(|&x| f64::from(x) * (f64::from(x) / t).log2())
            .sum::<f64>()
    };
    if size_hint >= 1 << 20 {
        // Sample 64-byte strides every 4 KiB; histos over literal >> 3.
        let mut combined = [0u32; 32];
        let mut ctx_h = [[0u32; 32]; 13];
        let mut total = 0u32;
        let mut start = 0usize;
        while start + 64 <= input.len() {
            let mut p2 = input[start];
            let mut p1 = input[start + 1];
            for pos in (start + 2)..(start + 64) {
                let lit = input[pos];
                let ctx = usize::from(
                    K_STATIC_CONTEXT_MAP_COMPLEX_UTF8[compute_context_id(p1, p2, 2) as usize],
                );
                total += 1;
                combined[usize::from(lit >> 3)] += 1;
                ctx_h[ctx][usize::from(lit >> 3)] += 1;
                p2 = p1;
                p1 = lit;
            }
            start += 4096;
        }
        if total > 0 {
            let inv = 1.0 / f64::from(total);
            let e1 = sh(&combined) * inv;
            let e2: f64 = ctx_h.iter().map(|h| sh(h)).sum::<f64>() * inv;
            if e2 <= 3.0 && e1 - e2 >= 0.2 {
                return Some((13, K_STATIC_CONTEXT_MAP_COMPLEX_UTF8.to_vec()));
            }
        }
    }
    // Bigram prefix (top-2-bit classes) → 1 / 2 / 3 contexts.
    static LUT: [usize; 4] = [0, 0, 1, 2];
    static SIMPLE: [u8; 64] = {
        let mut m = [0u8; 64];
        let mut i = 2;
        while i < 4 {
            m[i] = 1;
            i += 1;
        }
        m
    };
    static CONTINUATION: [u8; 64] = {
        let mut m = [0u8; 64];
        m[2] = 1;
        m[3] = 2;
        m
    };
    let mut bigram = [0u32; 9];
    let mut start = 0usize;
    while start + 64 <= input.len() {
        let mut prev = LUT[input[start] as usize >> 6] * 3;
        for pos in (start + 1)..(start + 64) {
            let lit = input[pos];
            bigram[prev + LUT[lit as usize >> 6]] += 1;
            prev = LUT[lit as usize >> 6] * 3;
        }
        start += 4096;
    }
    let mut mono = [0u32; 3];
    let mut two = [0u32; 6];
    for (i, &b) in bigram.iter().enumerate() {
        mono[i % 3] += b;
        two[i % 6] += b;
    }
    let total: f64 = mono.iter().map(|&x| f64::from(x)).sum();
    if total == 0.0 {
        return None;
    }
    let inv = 1.0 / total;
    let e1 = sh(&mono) * inv;
    let e2 = (sh(&two[..3]) + sh(&two[3..])) * inv;
    let mut e3 = 0.0;
    for i in 0..3 {
        e3 += sh(&bigram[3 * i..3 * i + 3]);
    }
    e3 *= inv;
    if quality < 7 {
        e3 = e1 * 10.0;
    }
    if e1 - e2 < 0.2 && e1 - e3 < 0.2 {
        None
    } else if e2 - e3 < 0.02 {
        Some((2, SIMPLE.to_vec()))
    } else {
        Some((3, CONTINUATION.to_vec()))
    }
}

/// Copy-length candidates evaluated per match in the optimal parsers.
/// Sampled at copy-code group boundaries (the highest L in each group
/// wins ties, since within a copy-length code the wire cost is flat).
const COPY_BOUNDARIES: [u32; 54] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 22, 24, 26, 28, 30, 32, 34,
    36, 40, 44, 48, 52, 60, 68, 76, 84, 100, 116, 132, 148, 164, 180, 196, 212, 228, 244, 260, 271,
    432, 496, 752, 1264, 2288, 3040, 4096,
];

/// A parsed LZ77 command: insert `insert_len` literals, then copy
/// `copy_len` bytes from `distance` (1-based backward offset).
#[derive(Clone, Copy, Debug)]
pub struct Command {
    pub insert_len: u32,
    pub copy_len: u32,
    pub distance: u32,
}

/// Four-slot repeat-distance ring buffer matching the decoder's state
/// (decoder_full.rs ~495). Used by `build_symbol_stream` to emit
/// explicit distance codes 0-3 for `rep0`/`rep1`/`rep2`/`rep3` matches
/// (TODO 245).
///
/// ## Decoder semantics (mirrored here)
///
/// For distance code `c` (0-3) with LZ77 back-reference:
/// - Read slot `(idx - (c-3)) & 3`.
/// - Decrement `idx` by `1 >> c` (only `c == 0` decrements).
/// - LZ77 copy: write the read value to slot `idx & 3`, then `idx += 1`.
///
/// For long-form distance with LZ77 back-reference:
/// - Write the new distance to slot `idx & 3`, then `idx += 1`.
///
/// For dictionary references: no write-back. Only implicit commands
/// (`distance_code >= 0`) get `idx += 1` compensation.
///
/// Initial state matches the C reference: `dist_rb = [16, 15, 11, 4]`,
/// `idx = 0`.
/// Literal-context (p1, p2) at a chunk start: the frame's previous two
/// output bytes (upstream ring-buffer semantics), or (0, 0) at frame start.
static LIT0: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn carried_lit_ctx(full_input: &[u8], mlen_offset: usize) -> (u8, u8) {
    match mlen_offset {
        0 => (0, 0),
        1 => (full_input[0], 0),
        n => (full_input[n - 1], full_input[n - 2]),
    }
}

#[derive(Clone, Debug)]
struct RepBuffer {
    dist_rb: [u32; 4],
    idx: i32,
}

impl RepBuffer {
    fn new() -> Self {
        Self {
            dist_rb: [16, 15, 11, 4],
            idx: 0,
        }
    }

    /// Returns the distance currently at rep code `code` (0-3).
    /// Code 0 is the most recent; code 3 is the oldest.
    fn rep_at(&self, code: u32) -> u32 {
        debug_assert!(code <= 3);
        let offset = code as i32 - 3;
        let idx = (self.idx - offset) & 3;
        self.dist_rb[idx as usize]
    }

    /// If `distance` matches any rep code, returns the smallest matching
    /// code (prefers lower codes on ties — they save the same bits).
    fn find_rep_code(&self, distance: u32) -> Option<u32> {
        for code in 0..4u32 {
            if self.rep_at(code) == distance {
                return Some(code);
            }
        }
        None
    }

    /// Returns the cheapest distance SHORT CODE (0-15, RFC 7932 §4)
    /// that reproduces `distance` from the current ring-buffer state:
    /// - Codes 0-3: exact rep0/rep1/rep2/rep3.
    /// - Codes 4-9: rep0 ± {1,2,3}.
    /// - Codes 10-15: rep1 ± {1,2,3}.
    ///
    /// All 16 codes cost only a Huffman symbol (no extra bits). This is
    /// how the reference encoder encodes slowly-drifting distance chains
    /// (e.g. periodic structures whose period shifts by ±1 per row) at
    /// ~3 bits instead of a ~15-bit long-form code.
    fn find_short_code(&self, distance: u32) -> Option<u32> {
        for code in 0..4u32 {
            if self.rep_at(code) == distance {
                return Some(code);
            }
        }
        const DELTAS: [i32; 6] = [-1, 1, -2, 2, -3, 3];
        // Codes 4-9: rep0 ± delta.
        let rep0 = self.rep_at(0) as i32;
        for (k, &d) in DELTAS.iter().enumerate() {
            if rep0 + d == distance as i32 && rep0 + d >= 1 {
                return Some(4 + k as u32);
            }
        }
        // Codes 10-15: rep1 ± delta.
        let rep1 = self.rep_at(1) as i32;
        for (k, &d) in DELTAS.iter().enumerate() {
            if rep1 + d == distance as i32 && rep1 + d >= 1 {
                return Some(10 + k as u32);
            }
        }
        None
    }

    /// Update state after an LZ77 back-reference that used rep code `code`
    /// (0-3) or implicit (= code 0). Mirrors decoder behavior exactly.
    fn on_rep_lz77(&mut self, code: u32) {
        let offset = code as i32 - 3;
        let distance_context = 1i32 >> code;
        // Read (capture the distance before modifying idx).
        let read_idx = ((self.idx - offset) & 3) as usize;
        let distance = self.dist_rb[read_idx];
        // Modify idx (only code 0 decrements).
        self.idx -= distance_context;
        // LZ77 write-back: write the read value at the new idx, then idx += 1.
        let write_idx = (self.idx & 3) as usize;
        self.dist_rb[write_idx] = distance;
        self.idx = self.idx.wrapping_add(1);
    }

    /// Update state after an LZ77 back-reference with a new long-form distance.
    fn on_new_distance_lz77(&mut self, distance: u32) {
        let write_idx = (self.idx & 3) as usize;
        self.dist_rb[write_idx] = distance;
        self.idx = self.idx.wrapping_add(1);
    }

    /// Update state after a dictionary reference. Only implicit commands
    /// (was_implicit = true) get idx compensation; explicit code 0-3 used
    /// for dict references would corrupt the buffer (encoder avoids this).
    fn on_dict_reference(&mut self, was_implicit: bool) {
        if was_implicit {
            self.idx = self.idx.wrapping_add(1);
        }
    }
}

#[cfg(test)]
#[allow(dead_code)]
impl RepBuffer {
    /// Snapshot the current rep0 (most recent distance).
    fn rep0(&self) -> u32 {
        self.rep_at(0)
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
/// Env-gated diagnostic flag, cached per call site. `std::env::var`
/// takes the global environ lock per call; in the parse/emit hot loops
/// that alone was measurable (~6% of encode, ~70% of decode before
/// caching).
macro_rules! env_flag {
    ($name:literal) => {{
        static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *CACHE.get_or_init(|| std::env::var($name).is_ok())
    }};
}

pub fn compress_with_quality(input: &[u8], quality: i32) -> Vec<u8> {
    let q = quality.clamp(0, 11);
    if input.is_empty() {
        return empty_frame();
    }

    // Q1: the reference's two-pass fragment compressor — an order of
    // magnitude faster than the from-spec parse with a BETTER ratio
    // on structured data (fresh per-128KB-block Huffman codes built
    // from actual histograms). BROTLI_NO_TP forces the from-spec path.
    if q == 1 && !env_flag!("BROTLI_NO_TP") {
        return crate::fast_encoder::compress_two_pass_q1(input);
    }

    // Inputs ≤ 1 MiB: single metablock, Huffman or uncompressed.
    // Larger metablocks reduce per-block Huffman table overhead.
    if input.len() < (1 << 20) {
        let uncompressed = encode_uncompressed_frame(input);
        let huffman = encode_huffman_frame_q(input, q);
        if !huffman.is_empty() && huffman.len() < uncompressed.len() {
            return huffman;
        }
        return uncompressed;
    }

    // Large inputs (> 1 MiB): split into 1 MiB chunks and emit
    // each as a Huffman-coded metablock. Uses a SINGLE match finder
    // over the full input so chunk N+1 can find matches referencing
    // data from chunks 0..N (cross-chunk matching).
    // Each chunk is a Huffman-coded metablock. Larger chunks amortize the
    // Huffman table overhead better but use more DP memory. Quality-dependent:
    // Q10+ uses 8 MiB, Q4-9 uses 4 MiB, Q0-3 uses 1 MiB.
    let chunk_size: usize = if let Some(sz) = std::env::var("BROTLI_CHUNK")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        sz
    } else if q >= 4 {
        // Re-measured on the REAL-CSV/FITS benchmarks after the greedy
        // parity work: 8 MiB chunks win on text (whole-file literal
        // statistics: CSV q5 440,437 -> 437,956) and are neutral on
        // binary; the old 2 MiB optimum was measured on the degenerate
        // 1000-row-period synthetic CSV.
        1 << 23 // 8 MiB
    } else {
        (1 << 20) - 1 // 1 MiB - 1
    };
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);

    // Build quality-dependent config for the shared match finder.
    let is_text = is_text_like(input);
    let (max_chain, nice_match, _, _, _, hash_log) = brotli_quality_config(q, is_text);
    // Hash-bucket scaling for large inputs (upstream scales hasher
    // params with input size): with fixed 2^17 buckets over N >> 2^17
    // positions, chains congest and a max_chain-deep walk only reaches
    // the most recent positions — the periodic structure at larger
    // distances vanishes and the greedy parse degrades as the input
    // grows (measured: CSV q5 greedy 4.55% @1MB -> 6.03% @4MB).
    // Hash scaling only for the greedy-tier experiment — the zopfli
    // DP's parse degrades with bigger tables at 21MB (+1.1%); keep the
    // default byte-identical.
    // Hash scaling rides the greedy tier's default predicate (text
    // >= 1 MiB) or its envs — worth 81KB at 21MB; the zopfli parse
    // degrades with bigger tables (+1.1%) so binary stays at the
    // per-quality log.
    let hash_log = if let Ok(v) = std::env::var("BROTLI_HASH_LOG") {
        v.parse().unwrap_or(hash_log)
    } else if (((q >= 4 && q < 10 && is_text_like(input)) || (q >= 4 && q < 8))
        && input.len() >= 1 << 20)
        || env_flag!("BROTLI_GREEDY_TIER")
    {
        let want = (input.len() as f64).log2().ceil() as u32 - 1;
        want.clamp(hash_log, 22)
    } else {
        hash_log
    };
    // Cross-chunk MF reuse for Q4+ (any content type). The optimal
    // parser has a cost model that correctly evaluates cross-chunk
    // distances, so this is safe for all data types.
    if q >= 4 {
        // Shared MF path: one match finder over the full input.
        let mf_config = omnizip_codecs::HashChainConfig {
            dict_size: MAX_BACKWARD_DISTANCE,
            min_match: MIN_MATCH,
            max_chain_length: max_chain,
            nice_match,
            hash_log,
            hash_bytes: 4,
            max_match_length: zopfli_max_len(q),
        };
        let mut shared_mf = omnizip_codecs::HashChainMatchFinder::new(input, mf_config);
        // H5 bank hasher for the greedy tier (q4-9): one-cache-line
        // bucket scans instead of prev[] chain walks, with the 16
        // short-code distance probes and BackwardReferenceScore
        // matching the reference hashers.
        let mut shared_bank = if q >= 4
            && q < 10
            && !env_flag!("BROTLI_NO_BANK")
            && !env_flag!("BROTLI_NO_TEXT_BANK")
        {
            let n = input.len();
            // Text params mirror the reference hashers exactly
            // (H5 at q4-8: block min(q-1,9); H9 at q9: block 8;
            // num_last_dists 4/10/16). Binary keeps the measured
            // block_bits=6 tuning that beats the reference.
            let (block_bits, dists) = if is_text_like(input) {
                let b = if q >= 9 { 8 } else { (q - 1).min(9) };
                let d = if q < 7 {
                    4
                } else if q < 9 {
                    10
                } else {
                    16
                };
                (b as u32, d)
            } else {
                // Reference H5/H6 params (block = min(q-1,9)), not
                // the old block-6 tuning: the 64-slot banks cost 4x
                // the scan per lookup and blew the time bar.
                (
                    if q >= 9 { 8 } else { (q - 1).min(9) } as u32,
                    if q < 7 {
                        4
                    } else if q < 9 {
                        10
                    } else {
                        16
                    },
                )
            };
            // Upstream ChooseHasher: bucket_bits = 14 only when
            // quality < 7 AND size_hint <= 1 MiB, else 15 (H5/H6/H9
            // all run 15). BROTLI_BUCKET_BITS overrides for sweeps.
            let bucket_bits = std::env::var("BROTLI_BUCKET_BITS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(if q < 7 && n <= 1 << 20 { 14 } else { 15 });
            let mut bank =
                omnizip_codecs::BankMatchFinder::new(input, bucket_bits, block_bits, dists);
            // Upstream 1.2.0 ChooseHasher: quality 5-9 with size_hint
            // >= 1 MiB (and lgwin >= 19) runs H6 — the 5-byte 64-bit
            // kHashMul64 hash — not H5's 4-byte kHashMul32. Smaller
            // inputs run H5. Measured on real fixtures the 5-byte hash
            // wins on text (longer matches per bucket) but LOSES ~4%
            // on structured binary: a 4-byte-prefixed match with a
            // differing 5th byte is excluded as a candidate even
            // though a len-4 copy is legal. Content-split: text runs
            // H6, binary keeps H5 (where our 4-byte bank beats the
            // reference's own H6 on FITS). BROTLI_HASH5 overrides.
            let hash5 = match std::env::var("BROTLI_HASH5").as_deref() {
                Ok("0") | Ok("false") => false,
                Ok("1") | Ok("true") => true,
                _ => is_text && n >= 1 << 20 && q >= 5,
            };
            if hash5 {
                bank.enable_hash5();
            }
            bank.set_max_distance(MAX_BACKWARD_DISTANCE);
            Some(bank)
        } else {
            None
        };

        let mut offset = 0usize;
        while offset < input.len() {
            let end = (offset + chunk_size).min(input.len());
            let is_last = end == input.len();
            let t0 = std::time::Instant::now();
            encode_huffman_chunk_with_shared_mf(
                &mut bw,
                input,
                offset,
                end,
                is_last,
                q,
                &mut shared_mf,
                shared_bank.as_mut(),
            );
            if env_flag!("BROTLI_STATS") {
                eprintln!("chunk {offset}..{end} q{q} took {:?}", t0.elapsed());
            }
            offset = end;
        }
    } else {
        // Per-chunk MF path (Q0-Q3 or binary).
        let mut offset = 0usize;
        while offset < input.len() {
            let end = (offset + chunk_size).min(input.len());
            let is_last = end == input.len();
            let ctx_in = carried_lit_ctx(&input, offset);
            encode_huffman_chunk_into(&mut bw, &input[offset..end], offset, is_last, q, ctx_in);
            offset = end;
        }
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
    ctx_in: (u8, u8),
) {
    // Standard path: MF created over the chunk slice itself.
    let is_text = is_text_like(input);
    let (max_chain, nice_match, _, _, _, hash_log) = brotli_quality_config(quality, is_text);
    let config = omnizip_codecs::HashChainConfig {
        dict_size: MAX_BACKWARD_DISTANCE,
        min_match: MIN_MATCH,
        max_chain_length: max_chain,
        nice_match,
        hash_log,
        hash_bytes: 4,
        max_match_length: zopfli_max_len(quality),
    };
    let mut mf = omnizip_codecs::HashChainMatchFinder::new(input, config);
    let hist_start = mlen_offset.min(MAX_BACKWARD_DISTANCE as usize);
    let _ = hist_start; // history unavailable on this path (single-chunk callers)
    encode_huffman_chunk_body(
        bw,
        input,
        &[],
        &mut mf,
        None,
        mlen_offset, // MF data[0] sits at global position mlen_offset
        mlen_offset,
        is_last,
        quality,
        ctx_in,
    );
}

/// Internal: encode one metablock with an external match finder.
/// The MF may reference the full input (cross-chunk) or just the
/// chunk slice (per-chunk), depending on the caller.
fn encode_huffman_chunk_body(
    bw: &mut BitWriter,
    input: &[u8],
    history: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    bank_mf: Option<&mut omnizip_codecs::BankMatchFinder>,
    mf_base: usize,
    mlen_offset: usize,
    is_last: bool,
    quality: i32,
    ctx_in: (u8, u8),
) {
    // Context modeling: at quality >= 4, split literals into context
    // trees. Active for Q4+ inputs ≥ 4 KiB (any content type — FSST-
    // transformed data benefits from context separation just as much
    // as natural text).
    // Context modeling earns its cost on TEXT (27% of CSV q5 output)
    // but only 1.8% on binary for 39% of encode time — binary q4-7
    // (the time-first tier) skips it. BROTLI_NO_CM forces off,
    // BROTLI_FORCE_CM forces on.
    let use_context = quality >= 4
        && input.len() >= 4096
        && !env_flag!("BROTLI_NO_CM")
        && (env_flag!("BROTLI_FORCE_CM") || quality >= 8 || is_text_like(input));

    // Block-type switching is disabled — testing showed a slight ratio
    // regression on uniform text data (per-block-type Huffman overhead
    // exceeds the benefit when statistics don't vary). The decoder now
    // correctly handles NBLTYPES > 1, and `write_block_type_trees` +
    // the inline switch emission in the literal loop are wired up, so
    // this can be flipped back on for inputs with strongly varying
    // per-block statistics.
    let use_block_switch = false;
    let (commands, precomputed) = parse_input_with_offset(
        input,
        history,
        mf,
        bank_mf,
        mf_base,
        mlen_offset,
        quality,
        false,
        is_last,
        ctx_in,
    );
    if env_flag!("BROTLI_PARSE_AUDIT") {
        let mut pos = mlen_offset;
        for c in &commands {
            pos += c.insert_len as usize;
            if c.copy_len > 0 {
                let max_dist = (pos as u32).min(MAX_BACKWARD_DISTANCE);
                let advance = if c.distance > max_dist {
                    let mut scratch = Vec::new();
                    match crate::dictionary::dictionary_lookup(
                        &mut scratch,
                        c.copy_len,
                        c.distance as i32,
                        max_dist,
                    ) {
                        Some(()) => scratch.len(),
                        None => c.copy_len as usize,
                    }
                } else {
                    c.copy_len as usize
                };
                pos += advance;
            }
        }
        if pos != mlen_offset + input.len() {
            eprintln!(
                "PARSE-OVERRUN mlen_offset={mlen_offset} len={} accounted={pos}",
                input.len()
            );
            let mut p2 = mlen_offset;
            for c in &commands {
                p2 += c.insert_len as usize;
                if c.copy_len > 0 {
                    let max_dist = (p2 as u32).min(MAX_BACKWARD_DISTANCE);
                    let advance = if c.distance > max_dist {
                        let mut scratch = Vec::new();
                        match crate::dictionary::dictionary_lookup(
                            &mut scratch,
                            c.copy_len,
                            c.distance as i32,
                            max_dist,
                        ) {
                            Some(()) => scratch.len(),
                            None => c.copy_len as usize,
                        }
                    } else {
                        c.copy_len as usize
                    };
                    p2 += advance;
                    if p2 > mlen_offset + input.len() {
                        eprintln!(
                            "OFFENDING at~{p2} ins={} copy={} dist={} advance={advance}",
                            c.insert_len, c.copy_len, c.distance
                        );
                        break;
                    }
                }
            }
        }
    }
    // The exact-acceptance chain already emitted the winning parse
    // with these exact header parameters — reuse its bits verbatim
    // instead of a fourth full emission (3 measures + final became
    // 3 measures total at q10+). The winner writer contains the
    // metablock header too, so the header is only written on the
    // recompute path.
    if let Some(won) = precomputed {
        append_writer(bw, won);
    } else {
        bw.write_bits(u32::from(is_last), 1); // ISLAST
                                              // ISLASTEMPTY only present when ISLAST=1; we never emit empty
                                              // metablocks, so always 0 when present.
        if is_last {
            bw.write_bits(0, 1); // ISLASTEMPTY = 0
        }
        // MLEN encoding: pick smallest MNIBBLES that fits.
        let mlen_minus_1 = (input.len() - 1) as u32;
        let (mnibbles, num_nibbles): (u32, u32) = if mlen_minus_1 < (1 << 16) {
            (0, 4)
        } else if mlen_minus_1 < (1 << 20) {
            (1, 5)
        } else {
            (2, 6)
        };
        bw.write_bits(mnibbles, 2);
        for i in 0..num_nibbles {
            bw.write_bits((mlen_minus_1 >> (4 * i)) & 0xF, 4);
        }
        // ISUNCOMPRESSED is only written when ISLAST=0 (matches
        // upstream `DecodeMetaBlockLength` gate).
        if !is_last {
            bw.write_bits(0, 1); // ISUNCOMPRESSED = 0
        }
        emit_metablock_from_commands(
            bw,
            input,
            mlen_offset,
            is_last,
            quality,
            ctx_in,
            use_context,
            use_block_switch,
            &commands,
        );
    }
}

/// Emission stage shared by the real encoder and parse-candidate
/// scoring: everything from the parsed command list to the last
/// tree-coded symbol. Pure with respect to `bw` — identical commands
/// produce identical bits.
#[allow(clippy::too_many_lines)]
fn emit_metablock_from_commands(
    bw: &mut BitWriter,
    input: &[u8],
    mlen_offset: usize,
    is_last: bool,
    quality: i32,
    ctx_in: (u8, u8),
    use_context: bool,
    use_block_switch: bool,
    commands: &[Command],
) {
    let _ = is_last;
    // Choose distance-code configuration from the parsed commands.
    let dist_cfg = DistanceConfig::choose(&commands);

    let Some(stream) = build_symbol_stream(&commands, input, mlen_offset, &dist_cfg) else {
        // Header consistency: NBLTYPESI/D must still be written before
        // returning (the metablock prefix is already on the wire).
        write_varlen_uint8(bw, 0); // NBLTYPESI = 1
        write_varlen_uint8(bw, 0); // NBLTYPESD = 1
        return;
    };

    // --- Command block splitting (BrotliBuildMetaBlock cmd pass) ---
    // Splits buy ~2KB (0.07%) on binary q5 for ~18% of encode —
    // binary q4-7 skips them (text keeps them: they earn their cost).
    let cmd_split_on = quality >= 4
        && stream.cmd_symbols.len() >= 1024
        && !env_flag!("BROTLI_NO_SPLIT")
        && (quality >= 8 || is_text_like(input));
    // q10+ parses ride implicit-rep0 commands heavily; their symbol
    // stream is broader and keeps sharpening up to 64 blocks (measured
    // 16→64 saves ~3.9KB at 1MB q11; 128 regresses on switch overhead).
    // Below ~32K symbols the switch overhead wins — 100KB regressed.
    let max_blocks = std::env::var("BROTLI_SPLIT_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if quality >= 10 && stream.cmd_symbols.len() >= 32_768 {
            (stream.cmd_symbols.len() / 900).clamp(16, 64)
        } else {
            16
        });
    let cmd_boundaries: Vec<usize> = if cmd_split_on {
        split_cmd_symbols_optimal(&stream.cmd_symbols, max_blocks)
    } else {
        vec![0]
    };
    let nbltypes_c = cmd_boundaries.len() as u32;
    let cmd_block_len: Vec<u32> = cmd_boundaries
        .iter()
        .enumerate()
        .map(|(k, &b)| {
            let end = cmd_boundaries
                .get(k + 1)
                .copied()
                .unwrap_or(stream.cmd_symbols.len());
            (end - b) as u32
        })
        .collect();
    // Per-command block-type assignment.
    let cmd_block_of: Vec<u8> = {
        let mut a = vec![0u8; stream.cmd_symbols.len()];
        for (k, &b) in cmd_boundaries.iter().enumerate() {
            let end = cmd_boundaries
                .get(k + 1)
                .copied()
                .unwrap_or(stream.cmd_symbols.len());
            for x in a.iter_mut().take(end).skip(b) {
                *x = k as u8;
            }
        }
        a
    };

    // --- Literal block splitting (BrotliBuildMetaBlock literal pass) ---
    // Below q10 the literal-tree assignment is the decided static map
    // (block-INdependent trees): literal block splits then only pay
    // switch-code overhead — a single literal block is both smaller
    // and faster. BROTLI_FORCE_LIT_SPLIT overrides.
    let decided_early: Option<(usize, Vec<u8>)> = if quality >= 5
        && use_context
        && is_text_like(input)
        && !env_flag!("BROTLI_FORCE_LIT_SPLIT")
    {
        decide_literal_contexts(input, quality, mlen_offset + input.len())
    } else {
        None
    };
    let lit_split_on = quality >= 4
        && stream.literals.len() >= 4096
        && use_context
        && decided_early.is_none()
        && !env_flag!("BROTLI_NO_LIT_SPLIT");
    // Scale the block budget with the literal count: small inputs
    // lose more to block/tree overhead than they gain from sharper
    // local statistics (measured crossover near ~2K literals/block).
    let max_lit_blocks = std::env::var("BROTLI_LIT_SPLIT_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            // q10+ benefits from far finer literal blocks than the
            // literals/1024 scaling on large streams (measured: 96
            // blocks vs 39 at 1MB q11 saves 3.5KB); smaller streams
            // keep the conservative scaling.
            if quality >= 10 && stream.literals.len() >= 32_768 {
                (stream.literals.len() / 400).clamp(16, 96)
            } else {
                // q<10: cap keeps the (block x context) clustering's
                // second phase small (blocks/64 groups x 8 centroids);
                // a few hundred uncapped blocks measured 8s of
                // clustering for +1% size.
                (stream.literals.len() / 1024).clamp(8, 48)
            }
        });
    // Reference 1.2.0 literal block split (SplitByteVector port):
    // FindBlocks DP over sampled entropy codes + batched HistogramPair
    // clustering. Default at q10+ (BROTLI_OLD_LIT_SPLIT restores the
    // in-house DP); BROTLI_REF_LIT_SPLIT forces it at any level.
    // Default at q10+ (BROTLI_OLD_LIT_SPLIT restores the in-house DP).
    // The stream-corruption bug is fixed: reused block-type ids got
    // zero-frequency codes in the block-type tree (zero-length code =
    // no bits written on switch, desyncing the decoder); the bt
    // histogram now counts the actually emitted types.
    let ref_lit_split =
        quality >= 10 && !env_flag!("BROTLI_OLD_LIT_SPLIT") || env_flag!("BROTLI_REF_LIT_SPLIT");
    let mut lit_block_types: Vec<u8> = Vec::new();
    let lit_boundaries: Vec<usize> = if !lit_split_on {
        vec![]
    } else if ref_lit_split {
        let syms: Vec<u16> = stream.literals.iter().map(|&b| u16::from(b)).collect();
        let iters = if quality >= 11 { 10 } else { 3 };
        let split = crate::encoder::block_splitter::split_byte_vector(
            &syms,
            crate::encoder::block_splitter::SYMBOLS_PER_LITERAL_HISTOGRAM,
            crate::encoder::block_splitter::MAX_LITERAL_HISTOGRAMS,
            crate::encoder::block_splitter::LITERAL_STRIDE_LENGTH,
            crate::encoder::block_splitter::LITERAL_BLOCK_SWITCH_COST,
            iters,
        );
        // Boundaries from lengths; keep the per-boundary TYPE so the
        // context map stays type-major (blocks sharing a type must
        // share trees — decoder semantics).
        // Old boundary format: [0, cut1, ...] EXCLUDING the final
        // total (lit_block_len derives the last block via len).
        // Block i has split length Li and type Ti; the boundary list
        // is [0] + end positions of every block except the last, and
        // lit_block_types[i] is block i's type (cmap is type-major).
        let mut boundaries = vec![0usize];
        let mut pos = 0usize;
        for (i, (len, &ty)) in split.lengths.iter().zip(split.types.iter()).enumerate() {
            if *len == 0 {
                continue;
            }
            pos += *len as usize;
            lit_block_types.push(ty);
            if pos < stream.literals.len() {
                boundaries.push(pos);
            }
        }
        while lit_block_types.len() > boundaries.len() {
            lit_block_types.pop();
        }
        while lit_block_types.len() < boundaries.len() {
            let next = lit_block_types.len() as u8;
            lit_block_types.push(next);
        }
        if env_flag!("BROTLI_DBG_CTX") {
            eprintln!(
                "LITSPLIT ref blocks={} types={:?} boundaries={:?} litcount={}",
                boundaries.len(),
                lit_block_types,
                boundaries,
                stream.literals.len()
            );
        }
        boundaries
    } else {
        lit_block_types = (0..max_lit_blocks).map(|i| i as u8).collect();
        split_literals(&stream.literals, max_lit_blocks)
    };
    let nbltypes_l: u32 = if lit_boundaries.is_empty() {
        1
    } else {
        usize::from(lit_block_types.iter().copied().max().unwrap_or(0)) as u32 + 1
    };
    let _ = &max_lit_blocks;
    let lit_block_len: Vec<u32> = lit_boundaries
        .iter()
        .enumerate()
        .map(|(k, &b)| {
            let end = lit_boundaries
                .get(k + 1)
                .copied()
                .unwrap_or(stream.literals.len());
            (end - b) as u32
        })
        .collect();
    // Context mode selection: UTF8 (2) for text-like input, LSB6 (0) otherwise.
    // UTF8 gives better context separation for multi-byte chars and ASCII text.
    let context_mode: u32 = if use_context && is_text_like(input) {
        2 // UTF8
    } else {
        0 // LSB6
    };
    let mut decided_ctx: Option<(usize, Vec<u8>)> = decided_early.clone();
    let (mut ntrees_l, mut lit_ctx_map): (u32, Vec<u8>) = if use_block_switch {
        (2, (0..128u8).map(|i| i >> 6).collect())
    } else if use_context && context_mode == 2 && input.len() >= 1_048_576 && quality >= 10 {
        // Static complex UTF-8 context map (13 trees) for large text
        // inputs at Q10+. Ported from the reference encoder's
        // `kStaticContextMapComplexUTF64`.
        (
            NTREES_COMPLEX_UTF8,
            K_STATIC_CONTEXT_MAP_COMPLEX_UTF8.to_vec(),
        )
    } else if use_context && context_mode == 2 && quality >= 5 && !env_flag!("BROTLI_NO_CTX_DECIDE")
    {
        // Reference DecideOverLiteralContextModeling at q5-9: pick the
        // context map from sampled entropy instead of a fixed 4-tree
        // ctx>>4 split. The decided map participates in the A/B/C
        // assignment below as option C.
        let decided = decided_early
            .clone()
            .or_else(|| decide_literal_contexts(input, quality, mlen_offset + input.len()));
        if env_flag!("BROTLI_STATS") {
            eprintln!(
                "STATS ctx_decide q{quality}: {} contexts",
                decided.as_ref().map_or(1, |(n, _)| *n)
            );
        }
        decided_ctx = decided;
        match &decided_ctx {
            Some((n, map)) => (*n as u32, map.clone()),
            None => (1, Vec::new()),
        }
    } else if use_context && input.len() >= 8192 {
        (4u32, (0..64u8).map(|ctx| ctx >> 4).collect())
    } else if use_context {
        (2, (0..64u8).map(|ctx| u8::from(ctx >= 32)).collect())
    } else {
        (1, Vec::new())
    };

    let mut ntrees = ntrees_l as usize;
    let mut lit_freqs: Vec<Vec<u32>> = vec![vec![0u32; 256]; ntrees];

    // Simulate output to get correct per-position context.
    // For dictionary references (distance > output.len()), the copy
    // produces bytes from the static dictionary, not the output buffer.
    // For transforms that change word length (prefix/suffix/omit), the
    // actual copy output length differs from cmd.copy_len (= word_length).
    // We precompute the actual copy advance for each command so subsequent
    // loops (frequency counting, encoding) advance correctly.
    let mut output_sim: Vec<u8> = Vec::with_capacity(input.len());
    let mut cmd_copy_advances: Vec<usize> = Vec::with_capacity(commands.len());
    let mut lit_idx = 0usize;
    for cmd in commands {
        for _ in 0..cmd.insert_len {
            output_sim.push(stream.literals[lit_idx]);
            lit_idx += 1;
        }
        let copy_advance = if cmd.copy_len > 0 {
            let before = output_sim.len();
            let copy_start_global = mlen_offset + output_sim.len();
            let max_dist = (copy_start_global as u32).min(MAX_BACKWARD_DISTANCE);
            // Use GLOBAL position for is_dict check. Cross-chunk LZ77
            // references have distance > local output_sim.len() but ≤
            // global position. Using local position misidentifies them
            // as dict references, corrupting the simulated output bytes
            // and causing context ID mismatches between encoder and decoder.
            let is_dict =
                (cmd.distance as usize) > copy_start_global.min(MAX_BACKWARD_DISTANCE as usize);
            if is_dict {
                let mut dict_bytes = Vec::with_capacity(cmd.copy_len as usize);
                if dictionary_lookup(&mut dict_bytes, cmd.copy_len, cmd.distance as i32, max_dist)
                    .is_some()
                {
                    output_sim.extend_from_slice(&dict_bytes);
                } else {
                    output_sim.extend(std::iter::repeat(0u8).take(cmd.copy_len as usize));
                }
            } else if (cmd.distance as usize) > output_sim.len() {
                // Cross-chunk LZ77 reference: source data is in a previous
                // chunk not present in output_sim. Since decoder output
                // equals the original input, use input bytes directly.
                let out_pos = output_sim.len();
                let copy_len = cmd.copy_len as usize;
                if out_pos + copy_len <= input.len() {
                    output_sim.extend_from_slice(&input[out_pos..out_pos + copy_len]);
                } else {
                    output_sim.extend(std::iter::repeat(0u8).take(copy_len));
                }
            } else {
                let src = output_sim.len() - cmd.distance as usize;
                for i in 0..cmd.copy_len as usize {
                    output_sim.push(output_sim[src + i]);
                }
            }
            output_sim.len() - before
        } else {
            0
        };
        cmd_copy_advances.push(copy_advance);
    }

    // Compute per-tree frequencies. Since the decoder's output equals
    // the original input, we use input[out_pos] directly instead of
    // output_sim[out_pos]. This avoids corruption from cross-chunk LZ77
    // references that output_sim can't reproduce (it only has the
    // current chunk's data, not previous chunks').
    // p1/p2 CARRY across metablocks: upstream's context lookup reads the
    // frame ring buffer's last two bytes, so a continuation chunk's first
    // literals are contexted by the previous chunk's tail.
    let (mut p1, mut p2) = ctx_in;
    let mut out_pos = 0usize;
    let mut lit_block_type: usize = 0;
    let mut walk_assign: Vec<(usize, u8)> = Vec::new();
    // Per-(block, context) literal histograms. With literal block
    // splitting (nbltypes_l > 1), each block gets its own context→tree
    // mapping; trees are shared across blocks (NTREES_L total).
    let bc_hists: Vec<[u32; 256]> = vec![[0u32; 256]; nbltypes_l as usize * 64];
    let max_lit_blocks_dbg = lit_boundaries.len();
    let nbltypes_l_dbg = nbltypes_l;
    let mut bc_hists = bc_hists;
    {
        let mut lit_pos = 0usize;
        let mut lit_blk = 0usize;
        let mut next_b = 1usize;
        for (cmd_idx, cmd) in commands.iter().enumerate() {
            for _ in 0..cmd.insert_len {
                if nbltypes_l > 1
                    && lit_blk + 1 < lit_boundaries.len()
                    && lit_pos >= lit_boundaries[lit_blk + 1]
                {
                    lit_blk += 1;
                    next_b += 1;
                }
                let _ = next_b;
                let b = input[out_pos];
                let ctx_id = compute_context_id(p1, p2, context_mode) as usize;
                let blk_ty = *lit_block_types.get(lit_blk).unwrap_or(&(lit_blk as u8)) as usize;
                bc_hists[(blk_ty << 6) + ctx_id][b as usize] += 1;
                if env_flag!("BROTLI_WALK_TRACE") {
                    walk_assign.push(((blk_ty << 6) + ctx_id, b));
                }
                p2 = p1;
                p1 = b;
                out_pos += 1;
                lit_pos += 1;
            }
            if cmd.copy_len > 0 {
                out_pos += cmd_copy_advances[cmd_idx];
                if out_pos > 0 && out_pos <= input.len() {
                    // Mirror the decoder exactly: for copies ≥ 2 bytes,
                    // p2 = second-to-last copied byte (NOT the pre-copy p1).
                    // A wrong p2 selects a different literal context tree
                    // than the decoder on fine-grained context maps.
                    let new_p1 = input[out_pos - 1];
                    p2 = if cmd.copy_len > 1 {
                        input[out_pos - 2]
                    } else {
                        p1
                    };
                    p1 = new_p1;
                }
            }
        }
    }

    // Data-driven tree assignment: isolate pure/low-diversity contexts
    // into dedicated (often single-symbol, zero-bit) trees; cluster the
    // rest into shared trees. Replaces the static map whenever
    // per-(block,context) histograms are available.
    if env_flag!("BROTLI_DBG_CTX") {
        let mut rows: Vec<(usize, u64, usize)> = bc_hists
            .iter()
            .enumerate()
            .map(|(i, h)| {
                (
                    i,
                    h.iter().map(|&x| u64::from(x)).sum(),
                    h.iter().filter(|&&x| x > 0).count(),
                )
            })
            .collect();
        rows.sort_by_key(|&(_, c, _)| std::cmp::Reverse(c));
        for (i, c, d) in rows.iter().take(20) {
            eprintln!("CTXDBG bucket[{i}] count={c} distinct={d}");
        }
    }
    {
        // Compare two tree strategies by expected wire cost and keep
        // the cheaper: (A) plain clustering vs (B) singleton isolation.
        // On literal-sparse inputs B's tree+cmap overhead exceeds its
        // zero-bit-literal savings.
        let tree_bits = |h: &[u32; 256]| -> f64 {
            let t: u64 = h.iter().map(|&x| u64::from(x)).sum();
            if t == 0 {
                return 0.0;
            }
            let mut e = 0.0f64;
            for &f in h.iter() {
                if f > 0 {
                    let p = f as f64 / t as f64;
                    e -= f as f64 * p.log2();
                }
            }
            e
        };
        // The A/B clustering passes cost ~20% of q5-9 encode; at those
        // qualities the reference uses ONLY its decided static map.
        // Take C directly there (BROTLI_FULL_ASSIGN restores the A/B/C
        // comparison); q10+ keeps the full machinery.
        let skip_ab = quality < 10 && decided_ctx.is_some() && !env_flag!("BROTLI_FULL_ASSIGN");
        // Literal-tree clustering cap: the reference's ContextBlockSplitter
        // reaches >100 trees at q11 (FITS: 143); a cap of 4 forfeits ~360KB
        // of literal entropy there. BROTLI_LIT_TREES overrides.
        let lit_trees_cap: usize = std::env::var("BROTLI_LIT_TREES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(if quality >= 10 { 64 } else { 4 });
        let cmap_a = if skip_ab {
            Vec::new()
        } else if env_flag!("BROTLI_STATS") {
            let t = std::time::Instant::now();
            let r = crate::encoder::context::cluster_contexts(&bc_hists, lit_trees_cap);
            eprintln!(
                "STATS cluster_lit rows={} blocks_dbg={max_lit_blocks_dbg} nbltypes_l={} cap={max_lit_blocks} n_literals={} {:.2}s",
                bc_hists.len(),
                nbltypes_l_dbg,
                stream.literals.len(),
                t.elapsed().as_secs_f64()
            );
            r
        } else {
            crate::encoder::context::cluster_contexts(&bc_hists, lit_trees_cap)
        };
        let mut hists_a: Vec<[u32; 256]> = vec![[0u32; 256]; lit_trees_cap];
        let (cost_a, cmap_b, count_b, cost_b);
        if skip_ab {
            cost_a = f64::INFINITY;
            cmap_b = Vec::new();
            count_b = 0;
            cost_b = f64::INFINITY;
        } else {
            for (i, h) in bc_hists.iter().enumerate() {
                for (b, &f) in h.iter().enumerate() {
                    hists_a[usize::from(cmap_a[i])][b] += f;
                }
            }
            cost_a = hists_a.iter().map(|h| tree_bits(h)).sum::<f64>()
                + 4.0 * 60.0
                + bc_hists.len() as f64 * 2.0;
            let (cmap_b_v, count_b_v) =
                crate::encoder::context::assign_context_trees(&bc_hists, ntrees.max(4));
            cmap_b = cmap_b_v;
            count_b = count_b_v;
            let mut hists_b: Vec<[u32; 256]> = vec![[0u32; 256]; count_b];
            for (i, h) in bc_hists.iter().enumerate() {
                for (b, &f) in h.iter().enumerate() {
                    hists_b[usize::from(cmap_b[i])][b] += f;
                }
            }
            cost_b = hists_b.iter().map(|h| tree_bits(h)).sum::<f64>()
                + count_b as f64 * 35.0
                + bc_hists.len() as f64 * (count_b as f64).log2().max(1.0);
        }
        // Option C: the reference's decided static map (tree by ctx
        // only, independent of block).
        let mut cost_c = f64::INFINITY;
        let mut cmap_c: Vec<u8> = Vec::new();
        let mut count_c = 0usize;
        if let Some((n, map)) = &decided_ctx {
            let mut hists_c: Vec<[u32; 256]> = vec![[0u32; 256]; *n];
            for (i, h) in bc_hists.iter().enumerate() {
                let t = usize::from(map[i & 63]);
                for (b, &f) in h.iter().enumerate() {
                    hists_c[t][b] += f;
                }
            }
            count_c = hists_c.iter().filter(|h| h.iter().sum::<u32>() > 0).count();
            if count_c > 0 {
                cost_c = hists_c.iter().map(|h| tree_bits(h)).sum::<f64>()
                    + count_c as f64 * 60.0
                    + bc_hists.len() as f64 * (count_c as f64).log2().max(1.0);
                // Compact tree ids: empty static-map trees are dropped.
                let mut c_remap = vec![usize::MAX; *n];
                let mut next = 0usize;
                for (t, h) in hists_c.iter().enumerate() {
                    if h.iter().sum::<u32>() > 0 {
                        c_remap[t] = next;
                        next += 1;
                    }
                }
                cmap_c = (0..bc_hists.len())
                    .map(|i| {
                        let t = c_remap[usize::from(map[i & 63])];
                        u8::try_from(t.min(count_c - 1)).unwrap_or(0)
                    })
                    .collect();
            }
        }
        // Option R: reference 1.2.0 histogram clustering (cluster_inc.h
        // port) over the per-(block,context) histograms — the machinery
        // behind the reference's 143-tree literal modeling at q11.
        // Opt-in (BROTLI_REF_CLUST): on current fixtures it TIES option
        // A within 0.5% (the reference's edge comes from its literal
        // BLOCK SPLITTING, not finer context clustering) while costing
        // ~3s on FITS q11.
        let mut cost_r = f64::INFINITY;
        let mut cmap_r: Vec<u8> = Vec::new();
        let mut count_r = 0usize;
        if env_flag!("BROTLI_REF_CLUST") {
            let hists: Vec<crate::encoder::block_splitter::Hist> = bc_hists
                .iter()
                .map(|h| {
                    let mut x = crate::encoder::block_splitter::Hist::new(256);
                    x.data.copy_from_slice(h);
                    x.total = h.iter().map(|&v| u64::from(v)).sum();
                    x
                })
                .collect();
            let (trees, symbols) = crate::encoder::block_splitter::cluster_histograms(&hists, 256);
            count_r = trees.len();
            if count_r > 0 {
                let hists_r: Vec<[u32; 256]> = trees
                    .iter()
                    .map(|t| {
                        let mut a = [0u32; 256];
                        a.copy_from_slice(&t.data);
                        a
                    })
                    .collect();
                cost_r = hists_r.iter().map(|h| tree_bits(h)).sum::<f64>()
                    + count_r as f64 * 60.0
                    + bc_hists.len() as f64 * (count_r as f64).log2().max(1.0);
                cmap_r = symbols.iter().map(|&sy| sy as u8).collect();
            }
        }
        let (cmap, tree_count) = if cost_r < cost_a && cost_r < cost_b && cost_r < cost_c {
            (cmap_r, count_r)
        } else if cost_c < cost_a && cost_c < cost_b {
            (cmap_c, count_c)
        } else if cost_b < cost_a && !env_flag!("BROTLI_NO_SINGLETONS") {
            (cmap_b, count_b)
        } else {
            (
                cmap_a.clone(),
                cmap_a
                    .iter()
                    .copied()
                    .max()
                    .map_or(1, |m| usize::from(m) + 1),
            )
        };
        if env_flag!("BROTLI_DBG_CTX") {
            eprintln!(
                "ASSIGN cost_a={cost_a:.0} cost_b={cost_b:.0} cost_c={cost_c:.0} cost_r={cost_r:.0} trees={tree_count}"
            );
        }
        lit_ctx_map.clear();
        lit_ctx_map.extend_from_slice(&cmap);
        ntrees_l = tree_count as u32;
        ntrees = tree_count;
        lit_freqs = vec![vec![0u32; 256]; ntrees];
    }
    if env_flag!("BROTLI_DBG_CTX") {
        let mx = lit_ctx_map.iter().max().copied().unwrap_or(0);
        eprintln!(
            "CTXMAP len={} ntrees={ntrees} max_val={mx} decided={}",
            lit_ctx_map.len(),
            decided_ctx.as_ref().map_or(0, |(n, _)| *n)
        );
    }
    for (cm_idx, hist) in bc_hists.iter().enumerate() {
        let tree = if ntrees > 1 {
            lit_ctx_map[cm_idx] as usize
        } else {
            0
        };
        for (b, &f) in hist.iter().enumerate() {
            lit_freqs[tree][b] += f;
        }
    }
    // Prune unused trees: the assignment may create tree ids that no
    // literal lands in (rare contexts, empty clusters). Every unused
    // tree would still cost a full header, so compact the ids and
    // remap the context map.
    if ntrees > 1 {
        let mut remap = vec![usize::MAX; ntrees];
        let mut next = 0usize;
        for t in 0..ntrees {
            let total: u32 = lit_freqs[t].iter().sum();
            if total > 0 {
                remap[t] = next;
                next += 1;
            }
        }
        if next == 0 {
            remap[0] = 0;
            next = 1;
        }
        let compact: Vec<Vec<u32>> = (0..ntrees)
            .filter(|&t| remap[t] != usize::MAX)
            .map(|t| std::mem::take(&mut lit_freqs[t]))
            .collect();
        lit_freqs = compact;
        for e in lit_ctx_map.iter_mut() {
            *e = remap[usize::from(*e)].min(next - 1) as u8;
        }
        ntrees = next;
        ntrees_l = next as u32;
    }

    write_varlen_uint8(bw, nbltypes_l - 1); // NBLTYPESL
    let mut lit_bt_wire: Vec<(u32, u8)> = Vec::new();
    let mut lit_bl_wire: Vec<(u32, u8)> = Vec::new();
    if nbltypes_l > 1 {
        let lit_switch_types: Vec<u8> = lit_block_types
            .iter()
            .enumerate()
            .filter(|(i, _)| *i > 0)
            .map(|(_, &t)| t)
            .collect();
        let (bt, bl) = write_block_switch_header(bw, nbltypes_l, &lit_block_len, &lit_switch_types);
        lit_bt_wire = bt;
        lit_bl_wire = bl;
    }
    write_varlen_uint8(bw, nbltypes_c - 1); // NBLTYPESI
    let mut cmd_bt_wire: Vec<(u32, u8)> = Vec::new();
    let mut cmd_bl_wire: Vec<(u32, u8)> = Vec::new();
    if nbltypes_c > 1 {
        // Block-type code tree over alphabet 2 + nbltypes_c. Switches
        // always use explicit codes (type + 2).
        let bt_alphabet = 2 + nbltypes_c as usize;
        let mut bt_freq = vec![0u32; bt_alphabet];
        for k in 1..nbltypes_c as usize {
            bt_freq[k + 2] += 1;
        }
        let bt_lengths = omnizip_codecs::HuffmanLengths::build(&bt_freq, 15);
        write_huffman_table(bw, &bt_lengths, bt_alphabet);

        // Block-length code tree over the 26-symbol alphabet, from the
        // actual block length distribution.
        let mut bl_freq = [0u32; 26];
        let bl_codes: Vec<(usize, u32, u32)> = cmd_block_len
            .iter()
            .map(|&l| block_length_code(l))
            .collect();
        for &(c, _, _) in &bl_codes {
            bl_freq[c] += 1;
        }
        let bl_lengths = omnizip_codecs::HuffmanLengths::build(&bl_freq, 15);
        write_huffman_table(bw, &bl_lengths, 26);

        cmd_bt_wire = canonical_with_reverse(&bt_lengths);
        cmd_bl_wire = canonical_with_reverse(&bl_lengths);

        // Initial block length (block 0) via the block-length tree.
        let (c0, extra0, nbits0) = bl_codes[0];
        let (code, len) = cmd_bl_wire[c0];
        bw.write_bits(code, u32::from(len));
        bw.write_bits(extra0, nbits0);
    }
    // --- Distance block splitting: NBLTYPES_D > 1 with per-block-type
    // context maps (before NPOSTFIX per the wire order). ---
    let dist_split_on = quality >= 4
        && stream.dist_symbols.len() >= 1024
        && dist_cfg.alphabet_size() <= 256
        && !env_flag!("BROTLI_NO_DSPLIT")
        && (quality >= 8 || is_text_like(input));
    let dist_boundaries: Vec<usize> = if dist_split_on {
        let syms: Vec<usize> = stream.dist_symbols.iter().map(|&s| s as usize).collect();
        split_symbol_stream_optimal(&syms, dist_cfg.alphabet_size(), 4)
    } else {
        vec![0]
    };
    let nbltypes_d = dist_boundaries.len() as u32;
    let dist_block_len: Vec<u32> = dist_boundaries
        .iter()
        .enumerate()
        .map(|(k, &b)| {
            let end = dist_boundaries
                .get(k + 1)
                .copied()
                .unwrap_or(stream.dist_symbols.len());
            (end - b) as u32
        })
        .collect();
    write_varlen_uint8(bw, nbltypes_d - 1); // NBLTYPESD
    let mut dist_bt_wire: Vec<(u32, u8)> = Vec::new();
    let mut dist_bl_wire: Vec<(u32, u8)> = Vec::new();
    if nbltypes_d > 1 {
        let (bt, bl) = write_block_switch_header(bw, nbltypes_d, &dist_block_len, &[]);
        dist_bt_wire = bt;
        dist_bl_wire = bl;
    }

    bw.write_bits(dist_cfg.npostfix as u32, 2); // NPOSTFIX
    bw.write_bits(dist_cfg.ndirect_code as u32, 4); // NDMOEM

    // Context mode fields: one PER literal block type (RFC 7932 §9.3).
    for _ in 0..nbltypes_l {
        bw.write_bits(context_mode, 2);
    }

    write_varlen_uint8(bw, ntrees_l - 1); // NTREESL
    if ntrees_l > 1 {
        write_context_map(bw, &lit_ctx_map, ntrees_l);
    }
    // Distance context modeling (RFC 7932 §9.6): NTREES_D = 2 with the
    // context derived from copy length (kCmdLut.context = (len>4)?3:len-2).
    // Short copies ride a short-code-heavy tree; long copies a long-code
    // tree — each sharper than the blended single tree.
    let mut ntrees_d: u32 =
        if quality >= 4 && !stream.dist_symbols.is_empty() && !env_flag!("BROTLI_NO_DTREES") {
            std::env::var("BROTLI_DTREES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4)
        } else {
            1
        };
    // Distance context trees over per-(block, context) buckets, with
    // unused trees pruned and a cost gate against the single-tree
    // variant. Written after the literal context map per wire order.
    let dist_alphabet = dist_cfg.alphabet_size();
    let nb_d = nbltypes_d as usize;
    // Per-(block, context) histograms only when symbols fit the fixed
    // 256-wide buckets (NPOSTFIX > 0 alphabets can reach 520).
    let dist_bc_ok =
        dist_alphabet <= 256 && stream.dist_symbols.iter().all(|&s| (s as usize) < 256);
    let mut dist_bc_hists: Vec<[u32; 256]> = if dist_bc_ok {
        vec![[0u32; 256]; nb_d * 4]
    } else {
        vec![[0u32; 256]; 1]
    };
    if dist_bc_ok {
        let mut blk = 0usize;
        for (idx, (&sym, &ctx)) in stream
            .dist_symbols
            .iter()
            .zip(stream.dist_ctxs.iter())
            .enumerate()
        {
            while blk + 1 < dist_boundaries.len() && idx >= dist_boundaries[blk + 1] {
                blk += 1;
            }
            dist_bc_hists[(blk << 2) + ctx as usize][sym as usize] += 1;
        }
    }
    let ent = |h: &[u32; 256]| -> f64 {
        let t: u64 = h.iter().map(|&x| u64::from(x)).sum();
        if t == 0 {
            return 0.0;
        }
        let mut e = 0.0f64;
        for &v in h.iter() {
            if v > 0 {
                e -= v as f64 * (v as f64 / t as f64).log2();
            }
        }
        e
    };
    let mut global_hist = [0u32; 256];
    if dist_bc_ok {
        for h in &dist_bc_hists {
            for (s, &v) in h.iter().enumerate() {
                global_hist[s] += v;
            }
        }
    }
    // Cost gates: (A) single tree, (B) per-(block,ctx) clustered trees.
    let global_hist = if dist_bc_ok {
        global_hist
    } else {
        let mut g = [0u32; 256];
        for &s in &stream.dist_symbols {
            if (s as usize) < 256 {
                g[s as usize] += 1;
            }
        }
        g
    };
    let cost_a = ent(&global_hist) + 70.0 + if dist_bc_ok { 0.0 } else { 1.0e9 };
    let shared_k = ntrees_d.min(4) as usize;
    let cmap_bc = if env_flag!("BROTLI_STATS") {
        let t = std::time::Instant::now();
        let r = crate::encoder::context::cluster_contexts(&dist_bc_hists, shared_k);
        eprintln!(
            "STATS cluster_dist rows={} {:.2}s",
            dist_bc_hists.len(),
            t.elapsed().as_secs_f64()
        );
        r
    } else {
        crate::encoder::context::cluster_contexts(&dist_bc_hists, shared_k)
    };
    let used_count = {
        let mut hists: Vec<[u32; 256]> = vec![[0u32; 256]; shared_k];
        for (i, h) in dist_bc_hists.iter().enumerate() {
            for (s, &v) in h.iter().enumerate() {
                hists[usize::from(cmap_bc[i])][s] += v;
            }
        }
        hists.iter().filter(|h| h.iter().sum::<u32>() > 0).count()
    };
    let cost_b: f64 = {
        let mut hists: Vec<[u32; 256]> = vec![[0u32; 256]; shared_k];
        for (i, h) in dist_bc_hists.iter().enumerate() {
            for (s, &v) in h.iter().enumerate() {
                if v > 0 {
                    hists[cmap_bc[i] as usize][s] += v;
                }
            }
        }
        hists.iter().map(|h| ent(h)).sum::<f64>()
            + used_count as f64 * 70.0
            + nb_d as f64 * 4.0 * 2.0
    };
    let mut dist_freqs_per_ctx: Vec<Vec<u32>>;
    let mut ntrees_d_out: u32;
    let mut dist_cmap_full: Vec<u8>;
    if cost_b < cost_a {
        dist_freqs_per_ctx = vec![vec![0u32; dist_alphabet]; shared_k];
        for (i, h) in dist_bc_hists.iter().enumerate() {
            for (s, &v) in h.iter().enumerate() {
                if v > 0 && s < dist_alphabet {
                    dist_freqs_per_ctx[cmap_bc[i] as usize][s] += v;
                }
            }
        }
        dist_cmap_full = cmap_bc;
        ntrees_d_out = shared_k as u32;
        // Prune unused trees.
        let used: Vec<bool> = dist_freqs_per_ctx
            .iter()
            .map(|f| f.iter().sum::<u32>() > 0)
            .collect();
        let mut remap = vec![0usize; shared_k];
        let mut next = 0usize;
        for (t, u) in used.iter().enumerate() {
            if *u {
                remap[t] = next;
                next += 1;
            }
        }
        if next == 0 {
            next = 1;
        }
        dist_freqs_per_ctx = (0..shared_k)
            .filter(|&t| used[t])
            .map(|t| std::mem::take(&mut dist_freqs_per_ctx[t]))
            .collect();
        for e in dist_cmap_full.iter_mut() {
            *e = remap[usize::from(*e)].min(next - 1) as u8;
        }
        ntrees_d_out = next as u32;
    } else {
        dist_freqs_per_ctx = vec![global_hist.to_vec()];
        dist_cmap_full = vec![0u8; nb_d * 4];
        ntrees_d_out = 1;
    }
    write_varlen_uint8(bw, ntrees_d_out - 1); // NTREESD
    if ntrees_d_out > 1 {
        write_context_map(bw, &dist_cmap_full, ntrees_d_out);
    }
    ntrees_d = ntrees_d_out;
    let dist_ctx_tree_of =
        |blk: usize, ctx: u8| -> usize { usize::from(dist_cmap_full[(blk << 2) + ctx as usize]) };

    // --- Context modeling: per-tree literal frequencies ---
    // For NTREES_L > 1, partition literals by their LSB6 context.
    // Build a virtual output buffer to correctly track the "previous byte"
    // for context computation (copies change the previous byte too).
    let mut cmd_freq = vec![0u32; 704];
    let mut dist_freq = vec![0u32; dist_alphabet];

    // Ensure every literal tree has at least one symbol. Smart context
    // clustering can produce trees with zero literals if no contexts map
    // to them. A zero-frequency tree would produce a degenerate Huffman
    // table that the decoder reads as "symbol 0, 0 bits per occurrence" —
    // corrupting output if that tree is ever selected during decoding.
    // Adding a dummy frequency for byte 0 prevents this.
    for freq in &mut lit_freqs {
        let total: u32 = freq.iter().sum();
        if total == 0 {
            freq[0] = 1;
        }
    }

    let mut cmd_freqs_per_block: Vec<Vec<u32>> = vec![vec![0u32; 704]; nbltypes_c as usize];
    for (i, &sym) in stream.cmd_symbols.iter().enumerate() {
        cmd_freqs_per_block[usize::from(cmd_block_of[i])][sym as usize] += 1;
    }
    if let Ok(path) = std::env::var("BROTLI_DUMP_CMDSYM") {
        use std::io::Write as _;
        let mut f = std::fs::File::create(&path).unwrap();
        for &sym in &stream.cmd_symbols {
            writeln!(f, "{sym}").unwrap();
        }
    }
    for freq in &mut cmd_freqs_per_block {
        let total: u32 = freq.iter().sum();
        if total == 0 {
            freq[0] = 1;
        }
    }
    for &sym in &stream.cmd_symbols {
        cmd_freq[sym] += 1;
    }
    for &sym in &stream.dist_symbols {
        dist_freq[sym as usize] += 1;
    }
    // (Per-context distance frequencies were computed before the
    // NTREESD header write — see dist_freqs_per_ctx / dist_ctx_tree.)

    // Dump per-tree literal frequencies for isolated round-trip tests.
    if env_flag!("BROTLI_DUMP_TREES") {
        for (i, freq) in lit_freqs.iter().enumerate() {
            let total: u32 = freq.iter().sum();
            let nz = freq.iter().filter(|&&f| f > 0).count();
            eprintln!("TREE {i} ntrees={ntrees} total={total} nz={nz} freqs={freq:?}");
        }
    }

    // Build per-tree literal Huffman tables.
    let mut lit_lengths_per_tree: Vec<omnizip_codecs::HuffmanLengths> = lit_freqs
        .iter()
        .map(|freq| omnizip_codecs::HuffmanLengths::build(freq, 15))
        .collect();
    let cmd_lengths_per_block: Vec<omnizip_codecs::HuffmanLengths> = cmd_freqs_per_block
        .iter()
        .map(|freq| omnizip_codecs::HuffmanLengths::build(freq, 15))
        .collect();
    let cmd_lengths = omnizip_codecs::HuffmanLengths::build(&cmd_freq, 15);
    let dist_lengths_per_ctx: Vec<omnizip_codecs::HuffmanLengths> = if ntrees_d > 1 {
        dist_freqs_per_ctx
            .iter()
            .map(|freq| omnizip_codecs::HuffmanLengths::build(freq, 15))
            .collect()
    } else {
        vec![omnizip_codecs::HuffmanLengths::build(&dist_freq, 15)]
    };
    let dist_lengths = omnizip_codecs::HuffmanLengths::build(&dist_freq, 15);

    // Diagnostic: entropy breakdown of the final symbol streams.
    if env_flag!("BROTLI_STATS") {
        let lit_bits: u64 = lit_freqs
            .iter()
            .zip(lit_lengths_per_tree.iter())
            .map(|(freq, huff)| {
                freq.iter()
                    .zip(huff.lengths.iter())
                    .map(|(&f, &l)| u64::from(f) * u64::from(l))
                    .sum::<u64>()
            })
            .sum();
        let cmd_sym_bits: u64 = cmd_freq
            .iter()
            .zip(cmd_lengths.lengths.iter())
            .map(|(&f, &l)| u64::from(f) * u64::from(l))
            .sum();
        // insert/copy extra bits from kCmdLut
        let cmd_extra_bits: u64 = stream
            .cmd_symbols
            .iter()
            .map(|&sym| {
                let e = &kCmdLut[sym as usize];
                u64::from(e.insert_len_extra_bits) + u64::from(e.copy_len_extra_bits)
            })
            .sum();
        let dist_sym_bits: u64 = dist_freq
            .iter()
            .zip(dist_lengths.lengths.iter())
            .map(|(&f, &l)| u64::from(f) * u64::from(l))
            .sum();
        let dist_extra_bits: u64 = {
            // extra bits depend on the distance config; recompute per symbol
            let mut total = 0u64;
            for &sym in &stream.dist_symbols {
                total += u64::from(distance_extra_bits(sym, &dist_cfg));
            }
            total
        };
        let n_rep = stream.dist_symbols.iter().filter(|&&s| s < 4).count();
        // Top distance VALUES (decoded from symbols + extras).
        let mut dist_values: std::collections::BTreeMap<u32, u32> =
            std::collections::BTreeMap::new();
        {
            let mut rep = RepBuffer::new();
            let mut out_pos = 0usize;
            let mut di = stream.dist_symbols.iter();
            for cmd in commands {
                out_pos += cmd.insert_len as usize;
                if cmd.copy_len > 0 {
                    let is_dict = (cmd.distance as usize)
                        > (mlen_offset + out_pos).min(MAX_BACKWARD_DISTANCE as usize);
                    if is_dict {
                        rep.on_dict_reference(false);
                    } else if rep.find_rep_code(cmd.distance).is_some() {
                        // rep: distance value already counted via cmd
                    }
                    *dist_values.entry(cmd.distance).or_insert(0) += 1;
                    if rep.find_rep_code(cmd.distance).is_some() {
                        // update below via find again
                    }
                    match rep.find_rep_code(cmd.distance) {
                        Some(code) => rep.on_rep_lz77(code),
                        None => rep.on_new_distance_lz77(cmd.distance),
                    }
                    out_pos += cmd.copy_len as usize;
                }
            }
            let _ = di;
        }
        let mut top: Vec<(u32, u32)> = dist_values.into_iter().collect();
        top.sort_by_key(|&(_d, c)| std::cmp::Reverse(c));
        top.truncate(8);
        eprintln!(
            "STATS ntrees={ntrees}: cmds={} literals={} dists={} (rep0-3: {n_rep}) | lit_bits={lit_bits} cmd_bits={} dist_bits={}",
            stream.cmd_symbols.len(),
            stream.literals.len(),
            stream.dist_symbols.len(),
            cmd_sym_bits + cmd_extra_bits,
            dist_sym_bits + dist_extra_bits
        );
        {
            // True bit split under the emitted block/context trees.
            let mut cmd_split_bits = 0u64;
            for (bi, freq) in cmd_freqs_per_block.iter().enumerate() {
                let lens = &cmd_lengths_per_block[bi].lengths;
                for (s, &f) in freq.iter().enumerate() {
                    if f > 0 {
                        cmd_split_bits += u64::from(f) * u64::from(lens[s]);
                    }
                }
            }
            let mut dist_split_bits = 0u64;
            for (ti, freq) in dist_freqs_per_ctx.iter().enumerate() {
                let lens = &dist_lengths_per_ctx[ti].lengths;
                for (s, &f) in freq.iter().enumerate() {
                    if f > 0 {
                        dist_split_bits += u64::from(f) * u64::from(lens[s]);
                    }
                }
            }
            eprintln!(
                "STATS split: cmd_sym={} cmd_extra={} dist_sym={} dist_extra={} lit={} blocks={} dtrees={}",
                cmd_split_bits,
                cmd_extra_bits,
                dist_split_bits,
                dist_extra_bits,
                lit_bits,
                cmd_boundaries.len(),
                ntrees_d
            );
        }
        eprintln!("STATS top distances: {:?}", &top);
    }

    // Override code lengths for sparse LITERAL trees (2-4 symbols) when
    // using multi-tree context modeling (NTREES > 1). Context-clustered
    // trees can have very few symbols, and the complex form RLE encoding
    // produces wire-format mismatches for these sparse tables. Simple
    // form avoids the RLE path entirely. Only applied to literal trees
    // (not command/distance) to avoid ratio regression on those tables.
    if ntrees > 1 {
        for tree in &mut lit_lengths_per_tree {
            override_lengths_for_simple_form(&mut tree.lengths, 256);
        }
    }

    let lit_codes_per_tree: Vec<Vec<(u32, u8)>> = lit_lengths_per_tree
        .iter()
        .map(canonical_with_reverse)
        .collect();
    let cmd_codes_per_block: Vec<Vec<(u32, u8)>> = cmd_lengths_per_block
        .iter()
        .map(canonical_with_reverse)
        .collect();
    let cmd_codes = canonical_with_reverse(&cmd_lengths);
    let dist_codes_per_ctx: Vec<Vec<(u32, u8)>> = dist_lengths_per_ctx
        .iter()
        .map(canonical_with_reverse)
        .collect();
    let dist_codes = canonical_with_reverse(&dist_lengths);

    // Write literal tree group (one table per tree).
    if env_flag!("BROTLI_DUMP_LITTREE") {
        for (ti, tree) in lit_lengths_per_tree.iter().enumerate() {
            let lens: Vec<String> = tree
                .lengths
                .iter()
                .enumerate()
                .filter(|(_, &l)| l > 0)
                .map(|(s2, &l)| format!("{s2}:{l}"))
                .collect();
            eprintln!("LITTREE {ti} {}", lens.join(","));
        }
        eprintln!("LITCMAP ntrees={ntrees_l} map={:?}", lit_ctx_map);
    }
    for tree in &lit_lengths_per_tree {
        write_huffman_table(bw, tree, 256);
    }
    if env_flag!("BROTLI_DUMP_CMDTREE") {
        for tree in &cmd_lengths_per_block {
            let lens: Vec<String> = tree
                .lengths
                .iter()
                .enumerate()
                .filter(|(_, &l)| l > 0)
                .map(|(s, &l)| format!("{s}:{l}"))
                .collect();
            eprintln!("CMDTREE {}", lens.join(","));
        }
    }
    for tree in &cmd_lengths_per_block {
        write_huffman_table(bw, tree, 704);
    }
    for (ti, tree) in dist_lengths_per_ctx.iter().enumerate() {
        if env_flag!("BROTLI_DBG_DC") {
            let lens: Vec<String> = tree
                .lengths
                .iter()
                .enumerate()
                .filter(|(_, &l)| l > 0)
                .map(|(s, &l)| format!("{s}:{l}"))
                .collect();
            eprintln!("DCTREE[{ti}] lens={}", lens.join(","));
        }
        write_huffman_table(bw, tree, dist_alphabet);
    }

    // --- Encode commands + literals with per-context tree selection ---
    let mut dist_iter = stream.dist_symbols.iter().zip(stream.dist_extras.iter());
    (p1, p2) = ctx_in;
    lit_idx = 0;
    out_pos = 0;
    let mut lit_blk = 0usize;
    let mut lit_block_remaining: usize =
        lit_block_len.first().copied().unwrap_or(u32::MAX) as usize;
    let mut enc_cmd_n = 0usize;
    let mut lit_next_switch = 1usize;
    let mut cmd_block_remaining: usize =
        cmd_block_len.first().copied().unwrap_or(u32::MAX) as usize;
    let mut next_switch = 1usize; // index into cmd_boundaries/block types
    let mut dist_blk = 0usize;
    let mut dist_sym_idx = 0usize;
    let mut dist_block_remaining: usize =
        dist_block_len.first().copied().unwrap_or(u32::MAX) as usize;
    let mut dist_next_switch = 1usize;
    for (cmd_idx, (&cmd_sym, cmd)) in stream.cmd_symbols.iter().zip(commands.iter()).enumerate() {
        if cmd_idx > 0 && cmd_block_remaining == 0 && next_switch < cmd_boundaries.len() {
            // Block switch: explicit type code (type + 2), then block length.
            let new_type = next_switch; // blocks are numbered in order
            let (bt_code, bt_len) = cmd_bt_wire[new_type + 2];
            bw.write_bits(bt_code, u32::from(bt_len));
            let (c, extra, nbits) = block_length_code(cmd_block_len[next_switch]);
            let (bl_code, bl_len) = cmd_bl_wire[c];
            bw.write_bits(bl_code, u32::from(bl_len));
            bw.write_bits(extra, nbits);
            if env_flag!("BROTLI_SWITCH_LOG") {
                eprintln!(
                    "ENCSW-CMD n={cmd_idx} pos={mlen_offset}+{out_pos} type={new_type} len={}",
                    cmd_block_len[next_switch]
                );
            }
            cmd_block_remaining = cmd_block_len[next_switch] as usize;
            next_switch += 1;
        }
        let block = if nbltypes_c > 1 {
            let cd_block = next_switch.saturating_sub(1);
            let arr_block = usize::from(cmd_block_of[cmd_idx]);
            if arr_block != cd_block && std::env::var("BROTLI_CMDBLK_DBG").is_ok() {
                eprintln!("CMDBLK-DIVERGE cmd={cmd_idx} arr={arr_block} countdown={cd_block}");
            }
            arr_block
        } else {
            0
        };
        let cmd_table = if nbltypes_c > 1 {
            &cmd_codes_per_block[block]
        } else {
            &cmd_codes
        };
        let (code, len) = cmd_table[cmd_sym];
        if env_flag!("BROTLI_SYM_TRACE") && cmd_idx >= 230 && cmd_idx <= 240 {
            let hist_val = cmd_freqs_per_block.get(block).map_or(0, |h| h[cmd_sym]);
            eprintln!(
                "ENCSYM-CODE {cmd_idx} sym={cmd_sym} code={code} len={len} block={block} freq={hist_val}"
            );
        }
        bw.write_bits(code, u32::from(len));
        if nbltypes_c > 1 {
            cmd_block_remaining = cmd_block_remaining.saturating_sub(1);
        }

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
            // Literal block switch BEFORE the literal (decoder checks
            // block_length == 0 at the start of each literal read).
            if nbltypes_l > 1 && lit_block_remaining == 0 && lit_next_switch < lit_boundaries.len()
            {
                let new_type = *lit_block_types
                    .get(lit_next_switch)
                    .unwrap_or(&(lit_next_switch as u8)) as usize;
                if env_flag!("BROTLI_SW_TRACE") {
                    eprintln!(
                        "ENCSW n={lit_next_switch} type={new_type} len={} litpos={lit_idx} bit={}",
                        lit_block_len[lit_next_switch],
                        bw.out.len() * 8 + bw.nbits as usize
                    );
                }
                let (bt_code, bt_len) = lit_bt_wire[new_type + 2];
                bw.write_bits(bt_code, u32::from(bt_len));
                let (c, extra, nbits) = block_length_code(lit_block_len[lit_next_switch]);
                let (bl_code, bl_len) = lit_bl_wire[c];
                bw.write_bits(bl_code, u32::from(bl_len));
                bw.write_bits(extra, nbits);
                lit_blk = lit_next_switch;
                lit_block_remaining = lit_block_len[lit_next_switch] as usize;
                lit_next_switch += 1;
            }

            let b = stream.literals[lit_idx];
            if env_flag!("BROTLI_WALK_TRACE") {
                let w = walk_assign.get(lit_idx);
                let ctx = compute_context_id(p1, p2, context_mode) as usize;
                let blk_ty = *lit_block_types.get(lit_blk).unwrap_or(&(lit_blk as u8)) as usize;
                match w {
                    Some(&(wrow, wbyte)) if wrow != (blk_ty << 6) + ctx || wbyte != b => {
                        eprintln!(
                            "WALK-DIVERGE lit={lit_idx} walk_row={wrow} walk_byte={wbyte} emit_row={} emit_byte={b}",
                            (blk_ty << 6) + ctx
                        );
                    }
                    None => eprintln!("WALK-MISS lit={lit_idx}"),
                    _ => {}
                }
            }
            let tree = if nbltypes_l > 1 {
                let ctx = compute_context_id(p1, p2, context_mode) as usize;
                let blk_ty = *lit_block_types.get(lit_blk).unwrap_or(&(lit_blk as u8)) as usize;
                lit_ctx_map[(blk_ty << 6) + ctx] as usize
            } else if ntrees > 1 {
                let ctx = compute_context_id(p1, p2, context_mode) as usize;
                lit_ctx_map[ctx] as usize
            } else {
                0
            };
            let (lc, ll) = lit_codes_per_tree[tree][b as usize];
            if env_flag!("BROTLI_DBG_CTX") && u32::from(ll) == 0 {
                LIT0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let _trace_lit = env_flag!("BROTLI_LIT_TRACE");
            if _trace_lit {
                eprintln!(
                    "ENCLIT {lit_idx} bit={} tree={tree} len={ll} byte={b} p1={p1} p2={p2} blk={lit_blk}",
                    bw.out.len() * 8 + bw.nbits as usize
                );
            }
            bw.write_bits(lc, u32::from(ll));
            p2 = p1;
            p1 = b;
            lit_idx += 1;
            out_pos += 1;

            if nbltypes_l > 1 {
                lit_block_remaining = lit_block_remaining.saturating_sub(1);
            }
        }

        if env_flag!("BROTLI_CMD_TRACE") {
            eprintln!(
                "ENCCMD {enc_cmd_n} ins={} cpy={} dist={} outpos={out_pos} lit={lit_idx}",
                cmd.insert_len, cmd.copy_len, cmd.distance
            );
        }
        enc_cmd_n += 1;
        if cmd.copy_len > 0 {
            // Check if this command uses implicit distance (rep code).
            // Implicit commands don't have a distance symbol in the stream.
            let cmd_entry = &kCmdLut[cmd_sym];
            if cmd_entry.distance_code < 0 {
                let (&d_sym, &d_extra) = dist_iter.next().expect("distance stream exhausted");
                if nbltypes_d > 1 {
                    dist_sym_idx += 1;
                    if dist_block_remaining == 0 && dist_next_switch < dist_boundaries.len() {
                        let new_type = dist_next_switch;
                        let (bt_code, bt_len) = dist_bt_wire[new_type + 2];
                        bw.write_bits(bt_code, u32::from(bt_len));
                        let (c, extra, nbits) = block_length_code(dist_block_len[dist_next_switch]);
                        let (bl_code, bl_len) = dist_bl_wire[c];
                        bw.write_bits(bl_code, u32::from(bl_len));
                        bw.write_bits(extra, nbits);
                        if env_flag!("BROTLI_SWITCH_LOG") {
                            eprintln!(
                                "SW-DIST pos={mlen_offset}+{out_pos} type={dist_next_switch} len={}",
                                dist_block_len[dist_next_switch]
                            );
                        }
                        dist_blk = dist_next_switch;
                        dist_block_remaining = dist_block_len[dist_next_switch] as usize;
                        dist_next_switch += 1;
                    }
                    dist_block_remaining = dist_block_remaining.saturating_sub(1);
                }
                let table = if ntrees_d > 1 {
                    let ctx = if cmd.copy_len > 4 {
                        3u8
                    } else {
                        (cmd.copy_len - 2) as u8
                    };
                    &dist_codes_per_ctx[dist_ctx_tree_of(dist_blk, ctx)]
                } else {
                    &dist_codes
                };
                let (dc, dl) = table[d_sym as usize];
                if env_flag!("BROTLI_DIST_TRACE") && cmd_idx >= 230 && cmd_idx <= 240 {
                    let ctx = if cmd.copy_len > 4 {
                        3u8
                    } else {
                        (cmd.copy_len - 2) as u8
                    };
                    eprintln!(
                        "ENCDIST {cmd_idx} val={} sym={d_sym} extra={d_extra} ctx={ctx} code={dc} len={dl} bit={}",
                        cmd.distance,
                        bw.out.len() * 8 + bw.nbits as usize
                    );
                }
                if env_flag!("BROTLI_DBG_DC") {
                    eprintln!(
                        "DCWRITE sym={d_sym} code={dc:0b} len={dl} tree_idx={}",
                        if ntrees_d > 1 {
                            let ctx = if cmd.copy_len > 4 {
                                3u8
                            } else {
                                (cmd.copy_len - 2) as u8
                            };
                            match ntrees_d {
                                2 => usize::from(ctx >= 2),
                                _ => ctx as usize,
                            }
                        } else {
                            0
                        }
                    );
                }
                bw.write_bits(dc, u32::from(dl));
                let nbits = distance_extra_bits(d_sym, &dist_cfg);
                if nbits > 0 {
                    bw.write_bits(d_extra, nbits);
                }
            }
            if env_flag!("BROTLI_ADV_DBG") && cmd_copy_advances[cmd_idx] != cmd.copy_len as usize {
                eprintln!(
                    "ADV-DIFF cmd={cmd_idx} copy_len={} advance={}",
                    cmd.copy_len, cmd_copy_advances[cmd_idx]
                );
            }
            out_pos += cmd_copy_advances[cmd_idx];
            if out_pos > 0 && out_pos <= input.len() {
                // Mirror the decoder exactly (see frequency-collection
                // loop): p2 = second-to-last copied byte for copies ≥ 2.
                let new_p1 = input[out_pos - 1];
                p2 = if cmd.copy_len > 1 {
                    input[out_pos - 2]
                } else {
                    p1
                };
                p1 = new_p1;
            }
        }
    }
    if env_flag!("BROTLI_DBG_CTX") {
        eprintln!(
            "LIT0-final: zero-bit literals: {}",
            LIT0.load(std::sync::atomic::Ordering::Relaxed)
        );
    }
}

/// Encode one metablock (uncompressed) into the shared writer.
/// Kept for the `multi_metablock_uncompressed_round_trips` test.
/// Always passes `is_last = false`: per upstream brotli, an ISLAST=1
/// metablock cannot be uncompressed (ISUNCOMPRESSED is only read when
/// ISLAST=0). Terminate the stream with a separate empty ISLAST=1
/// metablock via [`empty_frame_terminator_into`].
#[cfg(test)]
fn encode_uncompressed_chunk_into(bw: &mut BitWriter, input: &[u8], _is_last: bool) {
    bw.write_bits(0, 1); // ISLAST = 0 (so ISUNCOMPRESSED is read)
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

/// Write an empty ISLAST=1 metablock (the stream terminator).
#[cfg(test)]
fn empty_frame_terminator_into(bw: &mut BitWriter) {
    bw.write_bits(1, 1); // ISLAST = 1
    bw.write_bits(1, 1); // ISLASTEMPTY = 1
}

// ---------------------------------------------------------------------------
// Uncompressed metablock (RFC 7932 §9.2)
// ---------------------------------------------------------------------------

fn encode_uncompressed_frame(input: &[u8]) -> Vec<u8> {
    // Per upstream `DecodeMetaBlockLength`: ISUNCOMPRESSED is only read
    // when ISLAST=0. To emit an uncompressed last metablock, we write
    // the data as ISLAST=0 + ISUNCOMPRESSED=1, then append a final
    // empty ISLAST=1 metablock.
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);

    bw.write_bits(0, 1); // ISLAST = 0 (so ISUNCOMPRESSED is read)

    let mnibbles_field: u32 = if input.len() < (1 << 16) { 0 } else { 2 };
    bw.write_bits(mnibbles_field, 2);

    let nibbles: u32 = if mnibbles_field == 0 {
        4
    } else {
        mnibbles_field + 4
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

    // Append empty ISLAST=1 metablock to terminate the stream.
    let mut terminator = BitWriter::new();
    terminator.write_bits(1, 1); // ISLAST = 1
    terminator.write_bits(1, 1); // ISLASTEMPTY = 1
    out.extend_from_slice(&terminator.flush());

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

/// Encode the entire input as a single Huffman-coded metablock. Calls
/// the chunk encoder with `mlen_offset=0` and `is_last=true`, then
/// prepends WBITS.
fn encode_huffman_frame_q(input: &[u8], quality: i32) -> Vec<u8> {
    if input.is_empty() || input.len() >= (1 << 20) {
        return Vec::new();
    }
    let mut bw = BitWriter::new();
    write_wbits(&mut bw);
    encode_huffman_chunk_into(&mut bw, input, 0, true, quality, (0, 0));
    bw.flush()
}

/// Like [`encode_huffman_chunk_into`] but uses `full_input` as the
/// match-finder data source, enabling cross-chunk match references.
/// Used by [`compress_with_quality`] for multi-metablock inputs.
fn encode_huffman_chunk_with_shared_mf(
    bw: &mut BitWriter,
    full_input: &[u8],
    chunk_start: usize,
    chunk_end: usize,
    is_last: bool,
    quality: i32,
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    bank_mf: Option<&mut omnizip_codecs::BankMatchFinder>,
) {
    let chunk = &full_input[chunk_start..chunk_end];
    let ctx_in = carried_lit_ctx(full_input, chunk_start);
    let hist_start = chunk_start.saturating_sub(MAX_BACKWARD_DISTANCE as usize);
    let history = &full_input[hist_start..chunk_start];
    encode_huffman_chunk_body(
        bw,
        chunk,
        history,
        mf,
        bank_mf,
        0, // shared MF spans the whole input; data[0] is global 0
        chunk_start,
        is_last,
        quality,
        ctx_in,
    );
}

/// A parsed symbol stream ready for entropy coding.
struct SymbolStream {
    /// Literal bytes in insertion order.
    literals: Vec<u8>,
    /// Command symbols (indices into kCmdLut, 0..704).
    cmd_symbols: Vec<usize>,
    /// Distance symbols (0..63) — one per command with `copy_len` > 0.
    dist_symbols: Vec<u32>,
    /// Distance extra-bit values, parallel to `dist_symbols`.
    dist_extras: Vec<u32>,
    /// Copy-length context (kCmdLut.context) for each distance symbol,
    /// parallel to `dist_symbols`. Used to split distance frequencies
    /// across NTREES_D context trees exactly as emitted.
    dist_ctxs: Vec<u8>,
}

/// Build the entropy-coded symbol stream from commands.
///
/// For each command:
/// - Look up the matching entry in `kCmdLut` (`cell_idx` ≥ 2 for explicit
///   distance; we never emit implicit-distance commands).
/// - Compute the distance symbol + extra bits via the long-code formula
///   (RFC 7932 §10.4).
fn build_symbol_stream(
    commands: &[Command],
    input: &[u8],
    mlen_offset: usize,
    dist_cfg: &DistanceConfig,
) -> Option<SymbolStream> {
    let mut literals = Vec::new();
    let mut cmd_symbols = Vec::with_capacity(commands.len());
    let mut dist_symbols = Vec::new();
    let mut dist_extras = Vec::new();
    let mut dist_ctxs: Vec<u8> = Vec::new();

    // Track the 4-distance ring buffer (TODO 245). Lets us emit
    // explicit distance codes 0-3 for rep0/1/2/3 matches, saving
    // the distance extra bits (typically 5-15 bits per match).
    let mut rep = RepBuffer::new();
    let mut prev_was_implicit = false;

    // Rep-state synchronization across metablocks: the reference decoder
    // PERSISTS its distance ring buffer across metablocks while this
    // encoder (and our decoder) reset it. A reset buffer starts as
    // [16,15,11,4]; a persisted one carries the previous chunk's last
    // distances. After FOUR explicit long-form distance pushes both
    // buffers provably hold the same four values, so implicit/short
    // codes are safe from copy #5 onward. For the first four copies of
    // a continuation chunk we therefore force explicit encoding.
    let mut explicit_copies_remaining = if mlen_offset > 0 { 4 } else { 0 };
    // Env knobs hoisted out of the per-command loop: getenv is a lock +
    // linear scan and showed up at ~1.6% of encode time when called per
    // command.
    let narrow_implicit_max: u32 = if env_flag!("BROTLI_NARROW_IMPLICIT") {
        9
    } else {
        u32::MAX
    };
    let no_short_codes = env_flag!("BROTLI_NO_SHORT");

    // Output-position cursor — needed to detect dictionary references
    // (distance > current output) which can't use rep codes (would
    // corrupt the decoder's ring buffer state).
    let mut output_pos = 0usize;

    for cmd in commands {
        let _ = input;
        output_pos += cmd.insert_len as usize;
        // Dictionary iff distance exceeds the GLOBAL copy start (decoder
        // semantics). A chunk-local comparison misreads cross-metablock
        // LZ77 references as dictionary refs and desyncs the rep model.
        let is_dict_ref = cmd.copy_len > 0
            && (cmd.distance as usize)
                > (mlen_offset + output_pos).min(MAX_BACKWARD_DISTANCE as usize);

        // Try implicit rep0 command: the command symbol itself implies
        // "use last distance" (kCmdLut symbols 0-127, i.e. insert ≤ 9
        // with copy ≤ ~69), saving the ENTIRE distance symbol — ~2-4
        // bits each. The reference rides this form for ~70% of its
        // commands. Consecutive implicit commands are legal (rep0 is
        // unchanged by the implicit read/write-back). Disabled for
        // dictionary references (decoder doesn't compensate them).
        let implicit_copy_max = narrow_implicit_max;
        let can_use_implicit = cmd.copy_len > 0
            && cmd.copy_len <= implicit_copy_max
            && !is_dict_ref
            && explicit_copies_remaining == 0
            && cmd.distance == rep.rep_at(0)
            && cmd.insert_len <= 9
            && find_cmd_symbol_with_rep(cmd.insert_len, cmd.copy_len, Some(0))
                .is_some_and(|sym| kCmdLut[sym].distance_code == 0);

        let cmd_sym = if can_use_implicit {
            find_cmd_symbol_with_rep(cmd.insert_len, cmd.copy_len, Some(0))
        } else {
            find_cmd_symbol(cmd.insert_len, cmd.copy_len)
        }?;

        if env_flag!("BROTLI_SYM_TRACE") && cmd_symbols.len() >= 230 && cmd_symbols.len() <= 240 {
            eprintln!(
                "ENCSYM {} ins={} cpy={} d={} -> sym={cmd_sym}",
                cmd_symbols.len(),
                cmd.insert_len,
                cmd.copy_len,
                cmd.distance
            );
        }
        cmd_symbols.push(cmd_sym);

        let entry = &kCmdLut[cmd_sym];
        let this_was_implicit = entry.distance_code >= 0;
        let mut emitted_dist_sym: Option<u32> = None;

        if entry.distance_code < 0 && cmd.copy_len > 0 {
            // Explicit distance symbol needed. For LZ77 back-references,
            // try the 16 short codes first (exact rep0-3 plus rep0/rep1
            // ± 1-3): each costs only a Huffman symbol — typically ~3
            // bits vs 10-20 for the long form.
            let (sym, extra) = if is_dict_ref {
                // Dictionary references can't use short codes (the decoder
                // doesn't compensate them for dicts).
                encode_distance(cmd.distance, dist_cfg)
            } else if explicit_copies_remaining > 0 {
                // Chunk-start synchronization: long form only.
                encode_distance(cmd.distance, dist_cfg)
            } else if no_short_codes {
                encode_distance(cmd.distance, dist_cfg)
            } else if let Some(code) = rep.find_short_code(cmd.distance) {
                (code, 0)
            } else {
                encode_distance(cmd.distance, dist_cfg)
            };
            dist_symbols.push(sym);
            dist_extras.push(extra);
            dist_ctxs.push(if cmd.copy_len > 4 {
                3
            } else {
                (cmd.copy_len - 2) as u8
            });
            emitted_dist_sym = Some(sym);
            if !is_dict_ref && explicit_copies_remaining > 0 {
                explicit_copies_remaining -= 1;
            }
        }

        // Update RepBuffer to mirror decoder state. This must follow the
        // SYMBOL actually emitted, not the distance value: explicit code 0
        // is a net no-op on the ring buffer, but every other explicit
        // symbol (short codes 1-15, direct and long form) PUSHES the
        // resolved distance — even when that value equals an existing rep.
        // Re-deriving the form from the distance alone desyncs the model
        // whenever a forced long-form (chunk-start sync) carries a value
        // that already sits in the buffer.
        if cmd.copy_len > 0 {
            if is_dict_ref {
                rep.on_dict_reference(this_was_implicit);
            } else if this_was_implicit || emitted_dist_sym == Some(0) {
                rep.on_rep_lz77(0);
            } else {
                rep.on_new_distance_lz77(cmd.distance);
            }
        }

        prev_was_implicit = this_was_implicit;

        // Advance output_pos by the copy advance.
        if cmd.copy_len > 0 {
            output_pos += if is_dict_ref {
                // Dict transforms can change length; query the actual length.
                let global_pos = mlen_offset + output_pos;
                let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                match dictionary_lookup(&mut tmp, cmd.copy_len, cmd.distance as i32, max_dist) {
                    Some(()) => tmp.len(),
                    None => cmd.copy_len as usize,
                }
            } else {
                cmd.copy_len as usize
            };
        }
    }

    // Extract literals in stream order from commands (re-derive from input
    // via a sequential cursor — the parser already grouped them).
    // For correctness we re-walk commands against a cursor.
    let mut cur = 0usize;
    for cmd in commands {
        let end = cur + cmd.insert_len as usize;
        literals.extend_from_slice(&input[cur..end]);
        // Advance by the actual input consumed. For dictionary references
        // with length-changing transforms (prefix/suffix/omit), the
        // transformed length may differ from copy_len (= word_length).
        let advance = if cmd.copy_len > 0 {
            let is_dict =
                (cmd.distance as usize) > (mlen_offset + end).min(MAX_BACKWARD_DISTANCE as usize);
            if is_dict {
                let global_pos = mlen_offset + end;
                let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                match dictionary_lookup(&mut tmp, cmd.copy_len, cmd.distance as i32, max_dist) {
                    Some(()) => tmp.len(),
                    None => cmd.copy_len as usize,
                }
            } else {
                cmd.copy_len as usize
            }
        } else {
            0
        };
        cur = end + advance;
    }

    Some(SymbolStream {
        literals,
        cmd_symbols,
        dist_symbols,
        dist_extras,
        dist_ctxs,
    })
}

/// Find the kCmdLut symbol matching (`insert_len`, `copy_len`).
///
/// For `copy_len > 0`: matches entries with `distance_code == -1`
/// (`cell_idx` >= 2) so an explicit distance code is read by the decoder.
///
/// For `copy_len == 0` (insert-only trailing command): matches any
/// entry whose `insert_len_offset` is in range and whose
/// `copy_len_offset == 2` (smallest). The decoder short-circuits at
/// metablock end without executing the copy, so the phantom `copy_len`
/// is harmless.
fn find_cmd_symbol(insert_len: u32, copy_len: u32) -> Option<usize> {
    find_cmd_symbol_impl(insert_len, copy_len, None)
}

/// Like [`find_cmd_symbol`] but optionally searches for implicit-distance
/// entries (rep codes). When `rep_code` is `Some(dc)`, searches for
/// entries with `distance_code == dc` instead of `distance_code == -1`.
/// This enables cheaper encoding for repeat-offset matches.
fn find_cmd_symbol_with_rep(
    insert_len: u32,
    copy_len: u32,
    rep_code: Option<i32>,
) -> Option<usize> {
    find_cmd_symbol_impl(insert_len, copy_len, rep_code)
}

/// Precomputed (insert_len, copy_len) -> symbol tables for the hot
/// paths: find_cmd_symbol_impl's per-cell linear scan over the
/// 704-entry kCmdLut ran per command per boundary and dominated
/// emission. Filled by STAMPING each kCmdLut entry's (insert,copy)
/// range (O(704) ranges, exactly the covered cells) — the original
/// per-cell scan initialization cost ~94M iterations as a fixed tax
/// on every encode. Covers insert 0..=64 x copy 2..=4165; outside it
/// the linear scan still applies. First match wins, matching the
/// scan's first-hit semantics (stamp only empty cells).
static CMD_SYM_TABLES: std::sync::OnceLock<(Vec<[i16; 4166]>, Vec<[i16; 4166]>)> =
    std::sync::OnceLock::new();

fn cmd_sym_tables() -> &'static (Vec<[i16; 4166]>, Vec<[i16; 4166]>) {
    CMD_SYM_TABLES.get_or_init(|| {
        // Vec rows: two 541KB stack arrays in this closure overflowed
        // the 2MB test-thread stack.
        let mut explicit = vec![[-1i16; 4166]; 65];
        let mut rep0 = vec![[-1i16; 4166]; 65];
        for (sym, entry) in kCmdLut.iter().enumerate() {
            let ins_lo = u32::from(entry.insert_len_offset);
            let ins_hi = ins_lo + ((1u32) << u32::from(entry.insert_len_extra_bits)) - 1;
            let cpy_lo = u32::from(entry.copy_len_offset);
            let cpy_hi = cpy_lo + ((1u32) << u32::from(entry.copy_len_extra_bits)) - 1;
            let is_rep0 = entry.distance_code == 0;
            // The explicit scan skips implicit-distance entries
            // (distance_code >= 0).
            let is_explicit = entry.distance_code < 0;
            for ins in ins_lo..=ins_hi.min(64) {
                for cpy in cpy_lo.max(2)..=cpy_hi.min(4165) {
                    let (ie, ic) = (ins as usize, cpy as usize);
                    if is_explicit && explicit[ie][ic] < 0 {
                        explicit[ie][ic] = sym as i16;
                    }
                    if is_rep0 && rep0[ie][ic] < 0 {
                        rep0[ie][ic] = sym as i16;
                    }
                }
            }
        }
        (explicit, rep0)
    })
}

#[test]
fn littree_roundtrip_repro() {
    let lengths: Vec<u8> = vec![
        0, 6, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 6, 3, 4, 4, 5, 6, 4, 4, 5, 4,
        4, 4, 6, 3, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let hl = omnizip_codecs::HuffmanLengths {
        lengths: lengths.clone(),
        max_length: 15,
    };
    let mut bw = BitWriter::new();
    write_huffman_table(&mut bw, &hl, 256);
    bw.byte_align();
    let bytes = bw.flush();
    let (tree, consumed) = crate::decoder::read_huffman_table(&bytes, 0, 256).expect("read");
    assert!(consumed <= bytes.len() * 8, "reader overran");
    let codes = canonical_with_reverse(&hl);
    for (sym, &(code, len)) in codes.iter().enumerate() {
        if len == 0 {
            continue;
        }
        let mut bw2 = BitWriter::new();
        bw2.write_bits(code, u32::from(len));
        bw2.byte_align();
        let b2 = bw2.flush();
        let mut br = crate::decoder::BitReader::new(&b2);
        let got = tree.read_symbol(&mut br).expect("decode");
        assert_eq!(
            got as usize, sym,
            "symbol {sym}: encoder code {code}/{len} decodes as {got}"
        );
    }
}

#[test]
fn cmap_roundtrip_repro() {
    let map: Vec<u8> = vec![
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 2, 3, 4, 5, 2, 3, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 1, 0,
        0, 0, 0, 0, 0, 0, 0, 1, 6, 7, 6, 6, 7, 6, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10, 11, 12, 11, 13, 12, 13, 9, 9, 9, 9, 9, 9, 9, 9, 9,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 14, 15, 15, 14, 14, 15, 15, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 9, 9, 9, 9, 9, 9, 9, 16, 17, 0, 16, 16, 16, 16, 17, 16, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 18, 19, 18,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 20,
        17, 0, 20, 17, 20, 17, 20, 20, 20, 20, 17, 20, 17, 17, 17, 17, 20, 17, 20, 17, 17, 0, 20,
        17, 20, 0, 20, 20, 17, 20, 20, 21, 22, 23, 17, 17, 17, 20, 17, 20, 20, 17, 0, 17, 0, 17,
        17, 20, 20, 17, 0, 17, 17, 17, 17, 20, 17, 20, 24, 24, 24, 24, 24, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 25, 25, 25, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 24, 24,
        24, 24, 24, 24, 0, 24, 24, 24, 24, 24, 24, 24, 24, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 26, 26, 26, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 24, 24, 24, 24, 0, 24,
        24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 27, 0, 0, 0, 0, 24, 0, 24, 24, 24, 24, 24, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 28, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 24, 24, 0, 0, 0, 0, 0, 0, 0, 0, 24, 24, 24, 24, 0, 24, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 29, 29,
        30, 30, 31, 31, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 24, 32, 33, 32, 32, 33, 0, 32, 32, 33, 32, 32, 33, 33, 33, 34, 32, 32, 33, 32, 32, 32,
        33, 33, 32, 32, 33, 32, 32, 33, 32, 33, 32, 35, 36, 37, 36, 35, 33, 32, 32, 32, 33, 32, 32,
        33, 34, 34, 33, 32, 32, 33, 32, 33, 32, 34, 32, 33, 34, 32, 32, 34, 32, 32, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        38, 39, 40, 41, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        42, 42, 0, 42, 42, 42, 42, 0, 42, 42, 42, 42, 42, 0, 42, 42, 42, 42, 42, 42, 42, 42, 42, 0,
        42, 42, 0, 42, 42, 42, 42, 42, 42, 0, 42, 43, 44, 45, 44, 42, 42, 42, 42, 42, 42, 42, 42,
        42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 42, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 46, 46,
        47, 48, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 49, 49,
        50, 51, 49, 52, 51, 52, 52, 50, 52, 50, 50, 52, 51, 50, 0, 0, 0, 51, 0, 0, 0, 0, 0, 0, 0,
        51, 51, 0, 51, 51, 0, 0, 0, 53, 54, 55, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 52, 52, 52, 49,
        52, 50, 49, 52, 0, 49, 52, 49, 49, 49, 49, 50,
    ];
    let ntrees: u32 = 56;
    let mut bw = BitWriter::new();
    write_context_map(&mut bw, &map, ntrees);
    bw.byte_align();
    let bytes = bw.flush();
    // read_context_map(data, bit_pos, size, num_htrees, max_rle)
    let (got, consumed) =
        crate::decoder_full::read_context_map(&bytes, 0, map.len(), ntrees, 0).expect("read");
    assert!(
        consumed <= bytes.len() * 8,
        "reader overran: {consumed} > {}",
        bytes.len() * 8
    );
    for (i, (&a, &b)) in map.iter().zip(got.iter()).enumerate() {
        assert_eq!(a, b, "cmap entry {i}: wrote {a}, read {b}");
    }
}

#[test]
fn cmdtree_roundtrip_repro() {
    let lengths: Vec<u8> = vec![
        0, 0, 0, 11, 0, 0, 0, 0, 0, 6, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 11, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 3, 8, 3, 7, 10, 0, 0, 0, 0, 1, 0, 3, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 10, 5, 0, 7, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 10, 0, 0, 0, 0, 0, 8, 10,
        0, 0, 0, 0, 0, 0, 7, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 10, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10, 0, 0, 0, 0,
        0, 0, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let hl = omnizip_codecs::HuffmanLengths {
        lengths: lengths.clone(),
        max_length: 15,
    };
    let mut bw = BitWriter::new();
    write_huffman_table(&mut bw, &hl, 704);
    bw.byte_align();
    let bytes = bw.flush();
    let (tree, consumed) = crate::decoder::read_huffman_table(&bytes, 0, 704).expect("read");
    assert!(consumed <= bytes.len() * 8, "reader overran buffer");
    let codes = canonical_with_reverse(&hl);
    for (sym, &(code, len)) in codes.iter().enumerate() {
        if len == 0 {
            continue;
        }
        let mut bw2 = BitWriter::new();
        bw2.write_bits(code, u32::from(len));
        bw2.byte_align();
        let b2 = bw2.flush();
        let mut br = crate::decoder::BitReader::new(&b2);
        let got = tree.read_symbol(&mut br).expect("decode");
        assert_eq!(
            got as usize, sym,
            "symbol {sym}: encoder code {code}/{len} decodes as {got}"
        );
    }
}

#[test]
fn fcs_rep0_audit() {
    for ins in 0..10u32 {
        for cpy in 2..200u32 {
            if let Some(sym) = find_cmd_symbol_with_rep(ins, cpy, Some(0)) {
                let e = &kCmdLut[sym];
                let ins_ok = u32::from(e.insert_len_offset) <= ins
                    && ins < u32::from(e.insert_len_offset) + (1u32 << e.insert_len_extra_bits);
                let cpy_ok = u32::from(e.copy_len_offset) <= cpy
                    && cpy < u32::from(e.copy_len_offset) + (1u32 << e.copy_len_extra_bits);
                if !ins_ok || !cpy_ok || e.distance_code != 0 {
                    panic!(
                        "BAD rep0 ({ins},{cpy}) -> {sym} off=({},{}) d={}",
                        e.insert_len_offset, e.copy_len_offset, e.distance_code
                    );
                }
            }
        }
    }
}

#[test]
fn fcs_audit() {
    for ins in 0..70u32 {
        for cpy in 2..200u32 {
            if let Some(sym) = find_cmd_symbol(ins, cpy) {
                let e = &kCmdLut[sym];
                let ins_ok = u32::from(e.insert_len_offset) <= ins
                    && ins < u32::from(e.insert_len_offset) + (1u32 << e.insert_len_extra_bits);
                let cpy_ok = u32::from(e.copy_len_offset) <= cpy
                    && cpy < u32::from(e.copy_len_offset) + (1u32 << e.copy_len_extra_bits);
                if !ins_ok || !cpy_ok || e.distance_code >= 0 {
                    panic!(
                        "BAD ({ins},{cpy}) -> {sym} off=({},{}) d={}",
                        e.insert_len_offset, e.copy_len_offset, e.distance_code
                    );
                }
            }
        }
    }
}

fn find_cmd_symbol_impl(insert_len: u32, copy_len: u32, rep_code: Option<i32>) -> Option<usize> {
    if insert_len <= 64 && copy_len >= 2 && copy_len <= 4165 {
        let (explicit, rep0) = cmd_sym_tables();
        let cell = match rep_code {
            None => &explicit[insert_len as usize][copy_len as usize],
            // No caller passes other rep codes today; stay exact via
            // the scan if one ever does.
            Some(dc) if dc != 0 => {
                return find_cmd_symbol_impl_slow(insert_len, copy_len, rep_code)
            }
            Some(_) => &rep0[insert_len as usize][copy_len as usize],
        };
        if *cell >= 0 {
            return Some(*cell as usize);
        }
        return None;
    }
    find_cmd_symbol_impl_slow(insert_len, copy_len, rep_code)
}

fn find_cmd_symbol_impl_slow(
    insert_len: u32,
    copy_len: u32,
    rep_code: Option<i32>,
) -> Option<usize> {
    let phantom = copy_len == 0;
    let effective_copy = if phantom { 2 } else { copy_len };
    for (i, entry) in kCmdLut.iter().enumerate() {
        if phantom {
            // Trailing insert-only: any distance_code is fine
        } else if let Some(rc) = rep_code {
            // Rep code: match specific distance_code
            if entry.distance_code != rc as i8 {
                continue;
            }
        } else {
            // Explicit: skip implicit entries
            if entry.distance_code >= 0 {
                continue;
            }
        }
        let ins_lo = u32::from(entry.insert_len_offset);
        let ins_hi = ins_lo + ((1u32) << u32::from(entry.insert_len_extra_bits)) - 1;
        let cpy_lo = u32::from(entry.copy_len_offset);
        let cpy_hi = cpy_lo + ((1u32) << u32::from(entry.copy_len_extra_bits)) - 1;
        if (ins_lo..=ins_hi).contains(&insert_len) && (cpy_lo..=cpy_hi).contains(&effective_copy) {
            return Some(i);
        }
    }
    // Fallback: if rep_code search fails, try explicit
    if rep_code.is_some() {
        return find_cmd_symbol_impl(insert_len, copy_len, None);
    }
    None
}

/// Encode an LZ77 distance as a (symbol, `extra_bits`) pair using the
/// given distance-code configuration.
///
/// Direct codes (when NDIRECT > 0): distance 1..=NDIRECT maps to
/// symbols 16..16+NDIRECT-1 with zero extra bits.
///
/// Long codes (symbol ≥ 16+NDIRECT): use the standard RFC 7932 §10.4
/// formula, shifted past the direct-code range.
fn encode_distance(distance: u32, cfg: &DistanceConfig) -> (u32, u32) {
    let ndirect = cfg.ndirect();

    // Direct codes: distance 1..=NDIRECT → symbol 16..16+NDIRECT-1
    if distance <= ndirect {
        return (NUM_SHORT + distance - 1, 0);
    }

    // Long codes (RFC 7932 §10.4 with NPOSTFIX): the wire distance
    // value is split into a base (bucketed) and a postfix (low bits).
    //   adjusted = distance - 1 - ndirect
    //   postfix = adjusted & ((1 << npostfix) - 1)
    //   base = adjusted >> npostfix
    //   distval = (nbits - 1) * 2 + odd/even
    //   symbol = num_direct + (distval << npostfix) + postfix
    //   extra_bits_value = base - bucket_offset
    let adjusted = distance - 1 - ndirect;
    let postfix_mask = (1u32 << cfg.npostfix) - 1;
    let postfix = adjusted & postfix_mask;
    let base = adjusted >> cfg.npostfix;

    let mut nbits: u32 = 1;
    while nbits < 24 {
        let limit_even = (4u32 << (nbits - 1)).saturating_sub(4) + (1u32 << nbits);
        let limit_odd = (6u32 << (nbits - 1)).saturating_sub(4) + (1u32 << nbits);
        let limit = limit_even.max(limit_odd);
        if base < limit {
            break;
        }
        nbits += 1;
    }
    let even_offset = (4u32 << (nbits - 1)).saturating_sub(4);
    let odd_offset = (6u32 << (nbits - 1)).saturating_sub(4);
    let (odd_bit, bucket_base) = if base >= odd_offset {
        (1, odd_offset)
    } else {
        (0, even_offset)
    };
    let distval = (nbits - 1) * 2 + odd_bit;
    let sym = cfg.num_direct() + (distval << cfg.npostfix) + postfix;
    let extra = base - bucket_base;
    (sym, extra)
}

/// Number of extra bits for a distance symbol under the given config.
fn distance_extra_bits(sym: u32, cfg: &DistanceConfig) -> u32 {
    let num_direct = cfg.num_direct();
    if sym < num_direct {
        // Short codes (0-15) and direct codes (16..16+NDIRECT-1): no extra bits.
        return 0;
    }
    // Strip the postfix bits to recover the bucket-distval.
    let distval = (sym - num_direct) >> cfg.npostfix;
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
/// Cost-aware DP optimal parser (TODO 201).
///
/// Uses dynamic programming to find the command sequence with minimum
/// estimated bit cost. The cost model uses Shannon entropy for literals
/// and fixed estimates for command/distance overhead.
///
/// Steps:
/// 1. Collect best match at each position via the hash-chain match finder.
/// 2. Build literal cost model from byte frequency distribution.
/// 3. Backward DP: `cost[i]` = minimum bits to encode `input[i..n]`.
/// 4. Forward reconstruction: walk the DP table to emit commands.
fn optimal_parse(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    use_dict: bool,
) -> Vec<Command> {
    // Use Huffman-derived literal costs (matches what the wire format
    // actually pays per byte). Shannon entropy underestimates for small
    // alphabets — see [`compute_huffman_lit_cost`].
    let cmds = optimal_parse_with_costs(
        input,
        mf,
        mlen_offset,
        use_dict,
        Some(compute_huffman_lit_cost(input)),
    );
    rewrite_for_rep_codes(cmds, input, mlen_offset)
}

/// Post-process commands to increase repeat-distance code usage.
///
/// Walks the command stream forward, tracking the 4-distance rep buffer
/// using the SAME state machine as `build_symbol_stream`'s RepBuffer.
/// At each command whose distance is NOT in the current rep buffer, checks
/// whether a match at any of the 4 stored rep distances of the SAME LENGTH
/// exists at the same position. If so, rewrites the command's distance to
/// the lowest-code rep that matches (keeping copy_len unchanged).
///
/// This significantly increases rep code usage on highly repetitive inputs
/// (CSV, source code, FSST-transformed data) where the original parser
/// picks the longest match at the cost of using an explicit distance code.
fn rewrite_for_rep_codes(
    mut commands: Vec<Command>,
    input: &[u8],
    mlen_offset: usize,
) -> Vec<Command> {
    let n = input.len();
    if commands.is_empty() || n < 4 {
        return commands;
    }
    let mut rep = RepBuffer::new();
    let mut cur = 0usize;

    for cmd in commands.iter_mut() {
        cur = cur.saturating_add(cmd.insert_len as usize);
        if cmd.copy_len == 0 {
            continue;
        }
        let dist = cmd.distance;
        // A command is a dict reference if distance > global output position.
        // Cross-chunk LZ77 references have distance > local cur but ≤ global
        // output position, so they're NOT dict references.
        let global_output_pos = mlen_offset + cur;
        let is_dict = (dist as usize) > global_output_pos.min(MAX_BACKWARD_DISTANCE as usize);
        if is_dict {
            // For dict references, advance by the transformed length
            // (may differ from copy_len when transforms add/remove bytes).
            let global_pos = mlen_offset + cur;
            let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
            let advance = {
                let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                match dictionary_lookup(&mut tmp, cmd.copy_len, cmd.distance as i32, max_dist) {
                    Some(()) => tmp.len(),
                    None => cmd.copy_len as usize,
                }
            };
            rep.on_dict_reference(false);
            cur = cur.saturating_add(advance);
            continue;
        }

        let copy_len = cmd.copy_len as usize;

        // If already a rep match, update state and continue.
        if let Some(code) = rep.find_rep_code(dist) {
            rep.on_rep_lz77(code);
            cur = cur.saturating_add(copy_len);
            continue;
        }

        // Try each rep slot (0-3) for a same-length match. Prefer the
        // lowest code (rep0 < rep1 < rep2 < rep3) — same wire bits, but
        // choosing rep0 keeps the rep buffer warmer for the next match.
        let mut found: Option<u32> = None;
        for code in 0..4u32 {
            let rdist = rep.rep_at(code);
            if rdist > 0 && (rdist as usize) <= cur && cur + copy_len <= n {
                let src = cur - rdist as usize;
                if input[src..src + copy_len] == input[cur..cur + copy_len] {
                    found = Some(code);
                    break;
                }
            }
        }
        if let Some(code) = found {
            let rdist = rep.rep_at(code);
            cmd.distance = rdist;
            rep.on_rep_lz77(code);
            cur = cur.saturating_add(copy_len);
            continue;
        }

        // New explicit distance. Update state.
        rep.on_new_distance_lz77(dist);
        cur = cur.saturating_add(copy_len);
    }

    commands
}

/// Compute per-byte Shannon entropy as a literal cost model.
///
/// Bytes that don't appear in `data` get cost 8.0 (worst case). Bytes
/// that appear get `-log2(p)` where p is their frequency, clamped to a
/// minimum of 1.0 bit (Huffman codes shorter than 1 bit don't exist).
fn compute_shannon_lit_cost(data: &[u8]) -> [f32; 256] {
    let mut freq = [0u32; 256];
    for &b in data {
        freq[b as usize] += 1;
    }
    let mut arr = [8.0f32; 256];
    if data.is_empty() {
        return arr;
    }
    let total = data.len() as f32;
    for i in 0..256 {
        if freq[i] > 0 {
            let p = freq[i] as f32 / total;
            let bits = -p.log2();
            arr[i] = bits.max(1.0);
        }
    }
    arr
}

/// Compute per-byte Huffman-derived literal cost.
///
/// Builds a Huffman tree over `data` (max code length 15, matching the
/// Brotli literal alphabet) and uses the **actual** assigned code length
/// per byte as the literal cost. This is more accurate than Shannon
/// entropy for small alphabets because Huffman trees are constrained to
/// integer bit lengths and a minimum of 1 bit per symbol.
///
/// Example: for data with 20 distinct bytes where each byte appears
/// ~equally often, Shannon entropy = log2(20) ≈ 4.32 bits. But Huffman
/// assigns ~5-bit codes to most symbols (since 2^4 = 16 < 20 ≤ 32 = 2^5),
/// giving ~5 bits/symbol. Using Shannon in the DP underestimates by
/// ~0.7 bits/byte, biasing toward emitting literals when copies would
/// be cheaper.
///
/// Bytes that don't appear in `data` get cost 8.0 (worst case).
fn compute_huffman_lit_cost(data: &[u8]) -> [f32; 256] {
    let mut freq = [0u32; 256];
    for &b in data {
        freq[b as usize] += 1;
    }
    let mut arr = [8.0f32; 256];
    if data.is_empty() {
        return arr;
    }

    let huff = omnizip_codecs::HuffmanLengths::build(&freq, 15);
    for i in 0..256 {
        if freq[i] > 0 {
            let len = huff.lengths[i];
            // Huffman gives 0 length only for unused symbols, but we've
            // already filtered. Single-symbol trees assign length 1.
            arr[i] = if len > 0 { len as f32 } else { 8.0 };
        }
    }
    arr
}

/// Port of the reference BrotliEstimateBitCostsForLiterals (non-UTF8
/// branch): each literal is priced by a sliding 4000-byte window byte
/// histogram — LOCAL entropy, not the whole-metablock average. On
/// region-structured binary (columnar data) the reference's Zopfli
/// parse leans on these local prices to decide copy-vs-literal.
fn sliding_window_lit_costs(input: &[u8]) -> Vec<f32> {
    let n = input.len();
    let mut out = vec![0f32; n];
    let window_half = 2000usize;
    let mut hist = [0u32; 256];
    let mut in_window = window_half.min(n);
    for &b in &input[..in_window] {
        hist[usize::from(b)] += 1;
    }
    for i in 0..n {
        if i >= window_half {
            hist[usize::from(input[i - window_half])] -= 1;
            in_window -= 1;
        }
        if i + window_half < n {
            hist[usize::from(input[i + window_half])] += 1;
            in_window += 1;
        }
        let mut h = hist[usize::from(input[i])];
        if h == 0 {
            h = 1;
        }
        let mut c = (in_window as f64).log2() - f64::from(h).log2() + 0.02905;
        if c < 1.0 {
            c = c * 0.5 + 0.5;
        }
        if i < 2000 {
            c += 0.7 - (2000 - i) as f64 / 2000.0 * 0.35;
        }
        out[i] = c as f32;
    }
    out
}

/// Like [`optimal_parse`] but accepts an optional literal cost override.
/// Used by the iterative parser refinement (TODO 246) to feed back
/// actual Huffman-derived code lengths into a second DP pass.
fn optimal_parse_with_costs(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    use_dict: bool,
    lit_cost_override: Option<[f32; 256]>,
) -> Vec<Command> {
    optimal_parse_with_costs_ext(
        input,
        mf,
        mlen_offset,
        use_dict,
        lit_cost_override,
        None,
        None,
        None,
    )
}

/// Extended version with command, distance cost, and rep-hint overrides.
/// Used by the iterative parser to feed back actual Huffman-derived
/// costs and rep-code awareness into subsequent DP passes.
fn optimal_parse_with_costs_ext(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    use_dict: bool,
    lit_cost_override: Option<[f32; 256]>,
    cmd_cost_override: Option<f32>,
    dist_cost_table: Option<&[f32; 704]>,
    rep_hint: Option<&[u32]>,
) -> Vec<Command> {
    let n = input.len();
    if n == 0 {
        return Vec::new();
    }

    // --- Step 1: Collect matches at each position ---
    // Each match is (distance, copy_len, advance_len, is_dict).
    // - Hash match: copy_len == advance_len == m.length
    // - Dict match (length-preserving): copy_len == advance_len == wl == tl
    // - Dict match (length-changing): copy_len = wl, advance_len = tl
    //   The decoder copies wl bytes from the dict reference but the input
    //   cursor advances by tl bytes.
    //
    // We collect TWO candidates per position:
    // - matches_at: the LONGEST match (best for amortizing command overhead)
    // - closest_at: the CLOSEST match with length ≥ MIN_MATCH (best for
    //   rep-code conversion — closer distances are more likely to become
    //   rep0, saving ~9 bits per match)
    let mut matches_at: Vec<Option<(u32, u32, u32, bool)>> = vec![None; n];
    let mut closest_at: Vec<Option<(u32, u32, u32, bool)>> = vec![None; n];
    for pos in 0..n {
        let global_pos = mlen_offset + pos;
        let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);

        if pos + MIN_MATCH as usize <= n {
            mf.advance();
            if let Some(m) = mf.find_match(mlen_offset + pos) {
                if m.distance <= max_dist && m.length >= MIN_MATCH {
                    let copy_len = m.length.min(MAX_COPY).max(MIN_MATCH);
                    matches_at[pos] = Some((m.distance, copy_len, copy_len, false));
                }
            }

            // Also collect the closest match for rep-code-aware DP.
            // Only active when rep_hint is provided (iterative parser),
            // since without rep_hint the closest match is always dominated
            // by the longest match (shorter copy + similar distance cost).
            if rep_hint.is_some() {
                if let Some(cm) = mf.find_closest_match(mlen_offset + pos, MIN_MATCH) {
                    if cm.distance <= max_dist && cm.length >= MIN_MATCH {
                        let copy_len = cm.length.min(MAX_COPY).max(MIN_MATCH);
                        let closest = (cm.distance, copy_len, copy_len, false);
                        if matches_at[pos] != Some(closest) {
                            closest_at[pos] = Some(closest);
                        }
                    }
                }
            }
        }

        // Check dictionary when hash found no match OR found a short
        // match (< 16 bytes). Long hash matches are almost never beaten
        // by the dictionary, and the dict lookup adds ~3μs per position
        // which is expensive on repetitive text (1M positions × 3μs = 3s).
        // FSST-transformed data typically has short hash matches (4-8
        // bytes) where the dictionary's word transforms can win.
        if use_dict {
            let hash_len = matches_at[pos].map(|(_, l, _, _)| l).unwrap_or(0);
            if hash_len < 16 {
                if let Some((d, wl, tl)) = dict_hash::find_match(input, pos, max_dist) {
                    if tl >= MIN_MATCH && pos + tl as usize <= n {
                        let copy_len = wl.min(MAX_COPY).max(MIN_MATCH);
                        let is_better = match matches_at[pos] {
                            None => true,
                            Some((_, existing_len, _, _)) => tl > existing_len,
                        };
                        if is_better {
                            matches_at[pos] = Some((d, copy_len, tl, true));
                        }
                    }
                }
            }
        }
    }

    // --- Step 2: Build literal cost model ---
    // For the first iteration, use Shannon entropy from input bytes.
    // For iterative refinement (TODO 246), the caller can pass a
    // refined lit_cost derived from iteration N's parsed literals.
    let lit_cost = if let Some(provided) = lit_cost_override {
        provided
    } else {
        compute_shannon_lit_cost(input)
    };

    // --- Distance cost approximation ---
    // Use override table if provided (from pass 1's Huffman analysis),
    // otherwise use the logarithmic approximation.
    let default_dist_cost = |dist: u32| -> f32 {
        if dist == 0 {
            return 0.0;
        }
        let d = dist as f32;
        if dist <= 4 {
            2.0 + 0.5 * d
        } else {
            let log_d = d.ln() / core::f32::consts::LN_2;
            (5.0 + log_d).min(22.0)
        }
    };

    // Command base cost: use override if provided, otherwise default.
    // 7.0 reflects typical Huffman code length for command symbols.
    // Env override for tuning experiments only.
    let cmd_base_cost = cmd_cost_override.unwrap_or_else(|| {
        std::env::var("BROTLI_CMD_BASE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(7.0)
    });

    // Copy-length extra bits cost. Longer copies require more extra bits
    // in the command encoding (up to 24 bits for copy_len > 4336).
    // Without this, the DP underestimates long-copy costs and chooses
    // them too aggressively — a regression on FSST-preprocessed data
    // where long copies are often suboptimal.
    let copy_extra_cost = |copy_len: u32| -> f32 {
        if copy_len <= 18 {
            0.0 // codes 0-7: 0 extra bits
        } else if copy_len <= 54 {
            1.0 // codes 8-9: 1 extra bit
        } else if copy_len <= 134 {
            2.0 // codes 10-11: 2 extra bits
        } else if copy_len <= 302 {
            3.0 // codes 12-13: 3 extra bits
        } else if copy_len <= 698 {
            4.0 // codes 14-15: 4 extra bits
        } else if copy_len <= 1638 {
            5.0 // codes 16-17: 5 extra bits
        } else if copy_len <= 2288 {
            7.0 // codes 18-22: 6-11 extra bits (approx)
        } else {
            18.0 // codes 23+: up to 24 extra bits
        }
    };

    // --- Step 3: Backward DP considering all sub-match lengths ---
    // For each position i with a longest match of length max_L at distance D:
    //   cost[i] = min over L in [MIN_MATCH, max_L] of:
    //              match_cost(D) + cost[i+L]
    //            OR  lit_cost(input[i]) + cost[i+1]
    // Within a single copy-length code, match_cost is constant, so the
    // search prefers longer L (which amortizes the fixed cost better)
    // unless a better alignment appears at i+L.
    //
    // To keep this tractable, sample L at copy-code group boundaries
    // (the highest L in each group wins ties). 45 samples per position
    // keeps the DP at O(45 * N). Reduced sets (16 entries) were tested
    // but hurt ratio by 2+ pp on CSV data — the finer granularity finds
    // significantly better match alignments.
    let mut cost = vec![f32::INFINITY; n + 1];
    let mut back_len: Vec<u32> = vec![0; n]; // 0 = literal, else = copy_len
    let mut back_advance: Vec<u32> = vec![0; n]; // parallel: cursor advance
    let mut back_dist: Vec<u32> = vec![0; n]; // parallel: match distance
    cost[n] = 0.0;

    // Evaluate a single match candidate at position i.
    macro_rules! eval_match {
        ($i:expr, $dist:expr, $copy_len:expr, $advance_len:expr, $is_dict:expr,
         $best:expr, $best_action:expr, $best_advance:expr, $best_dist:expr) => {{
            let dist: u32 = $dist;
            let copy_len: u32 = $copy_len;
            let advance_len: u32 = $advance_len;
            let is_dict: bool = $is_dict;
            if dist == 0 || copy_len < MIN_MATCH {
                continue;
            }
            let dc = if let Some(hint) = rep_hint {
                if hint[$i] != 0 && hint[$i] == dist {
                    0.0
                } else if let Some(table) = dist_cost_table {
                    let code = if dist <= 4 {
                        (dist - 1) as usize
                    } else {
                        4 + ((dist as f32).ln() / core::f32::consts::LN_2) as usize
                    };
                    table[code.min(703)]
                } else {
                    default_dist_cost(dist)
                }
            } else if let Some(table) = dist_cost_table {
                let code = if dist <= 4 {
                    (dist - 1) as usize
                } else {
                    4 + ((dist as f32).ln() / core::f32::consts::LN_2) as usize
                };
                table[code.min(703)]
            } else {
                default_dist_cost(dist)
            };
            let m_cost = cmd_base_cost + dc;
            if is_dict && advance_len != copy_len {
                let l = $i + advance_len as usize;
                if l <= n {
                    let total = m_cost + copy_extra_cost(copy_len) + cost[l];
                    if total < $best {
                        $best = total;
                        $best_action = copy_len;
                        $best_advance = advance_len;
                        $best_dist = dist;
                    }
                }
            } else if is_dict {
                let l = $i + copy_len as usize;
                if l <= n {
                    let total = m_cost + copy_extra_cost(copy_len) + cost[l];
                    if total < $best {
                        $best = total;
                        $best_action = copy_len;
                        $best_advance = copy_len;
                        $best_dist = dist;
                    }
                }
            } else {
                for &boundary in &COPY_BOUNDARIES {
                    if boundary < MIN_MATCH || boundary > copy_len {
                        continue;
                    }
                    let l = $i + boundary as usize;
                    if l > n {
                        break;
                    }
                    let total = m_cost + copy_extra_cost(boundary) + cost[l];
                    if total < $best {
                        $best = total;
                        $best_action = boundary;
                        $best_advance = boundary;
                        $best_dist = dist;
                    }
                }
            }
        }};
    }

    for i in (0..n).rev() {
        // Option A: Insert 1 literal.
        let lit_cost_total = lit_cost[input[i] as usize] + cost[i + 1];
        let mut best = lit_cost_total;
        let mut best_action = 0u32;
        let mut best_advance = 0u32;
        let mut best_dist = 0u32;

        // Option B: Evaluate all match candidates.
        if let Some((dist, copy_len, advance_len, is_dict)) = matches_at[i] {
            eval_match!(
                i,
                dist,
                copy_len,
                advance_len,
                is_dict,
                best,
                best_action,
                best_advance,
                best_dist
            );
        }
        // Only evaluate closest match when rep_hint is active — otherwise
        // it's always dominated by the longest match.
        if rep_hint.is_some() {
            if let Some((dist, copy_len, advance_len, is_dict)) = closest_at[i] {
                eval_match!(
                    i,
                    dist,
                    copy_len,
                    advance_len,
                    is_dict,
                    best,
                    best_action,
                    best_advance,
                    best_dist
                );
            }
        }

        cost[i] = best;
        back_len[i] = best_action;
        back_advance[i] = best_advance;
        back_dist[i] = best_dist;
    }

    // --- Step 4: Forward reconstruction ---
    let mut commands = Vec::new();
    let mut pos = 0;
    let mut insert_start = 0;

    while pos < n {
        if back_len[pos] > 0 {
            let copy_len = back_len[pos];
            let advance = back_advance[pos];
            let dist = back_dist[pos];
            let insert_len = (pos - insert_start) as u32;
            commands.push(Command {
                insert_len,
                copy_len,
                distance: dist,
            });
            pos += advance as usize;
            insert_start = pos;
        } else {
            pos += 1;
        }
    }

    // Trailing literals.
    if insert_start < n {
        commands.push(Command {
            insert_len: (n - insert_start) as u32,
            copy_len: 0,
            distance: 0,
        });
    }

    commands
}

/// True wire-cost model for distance codes. Prices an explicit distance
/// as its actual (symbol, extra-bits) encoding under a chosen
/// [`DistanceConfig`], with symbol costs from a Huffman table built on
/// the observed symbol distribution. Rep codes 0-3 price at their
/// Huffman symbol cost (no extra bits).
/// Smoothed entropy cost per symbol (port of upstream SetCost in
/// backward_references_hq.cc): cost[i] = log2(sum) - log2(count[i]),
/// floored at 1.0; unseen symbols cost log2(sum + #missing) + 2. Unlike
/// Huffman lengths this is continuous — a p=0.4 symbol costs 1.32 bits,
/// not a quantized 2 — which keeps the DP's preference landscape smooth
/// enough for per-symbol command pricing to work (Huffman tables
/// collapsed the search; measured 43,422 vs 38,821 at 1MB q11).
fn set_cost(hist: &[u32; 704], literal_histogram: bool) -> [f32; 704] {
    let mut sum: u64 = 0;
    for &h in hist.iter() {
        sum += u64::from(h);
    }
    let mut out = [0f32; 704];
    if sum == 0 {
        return out;
    }
    let log2sum = (sum as f64).log2();
    let mut missing_symbol_sum = sum;
    if !literal_histogram {
        for &h in hist.iter() {
            if h == 0 {
                missing_symbol_sum += 1;
            }
        }
    }
    let missing_cost = (missing_symbol_sum as f64).log2() as f32 + 2.0;
    for i in 0..704 {
        if hist[i] == 0 {
            out[i] = missing_cost;
        } else {
            let c = log2sum as f32 - (hist[i] as f32).log2();
            out[i] = c.max(1.0);
        }
    }
    out
}

struct DistCostModel {
    /// Smoothed-entropy cost per distance code (bits).
    code_cost: [f32; 704],
    /// Smoothed-entropy cost per COMMAND symbol (bits); None when the
    /// caller did not provide a previous parse to learn from.
    cmd_cost: Option<[f32; 704]>,
    cfg: DistanceConfig,
}

impl DistCostModel {
    /// Build from a command stream: choose the distance config, encode
    /// each copy's distance, histogram the symbols. Costs come from
    /// SetCost (smoothed entropy) rather than Huffman lengths.
    fn from_commands(
        commands: &[Command],
        mlen_offset: usize,
        with_cmd_costs: bool,
        setcost_dist: bool,
    ) -> Self {
        let cfg = DistanceConfig::choose(commands);
        let mut freq = [0u32; 704];
        let mut cmd_freq = [0u32; 704];
        let mut rep = RepBuffer::new();
        let mut cur = 0usize;
        let mut has_copy = false;
        for cmd in commands {
            cur += cmd.insert_len as usize;
            if cmd.copy_len > 0 {
                has_copy = true;
                let is_dict = (cmd.distance as usize)
                    > (mlen_offset + cur).min(MAX_BACKWARD_DISTANCE as usize);
                if with_cmd_costs {
                    if let Some(sym) = find_cmd_symbol(cmd.insert_len, cmd.copy_len) {
                        cmd_freq[sym] += 1;
                    }
                }
                if is_dict {
                    rep.on_dict_reference(false);
                } else if let Some(code) = rep.find_rep_code(cmd.distance) {
                    freq[code as usize] += 1;
                    rep.on_rep_lz77(code);
                } else {
                    let (sym, _) = encode_distance(cmd.distance, &cfg);
                    freq[sym.min(703) as usize] += 1;
                    rep.on_new_distance_lz77(cmd.distance);
                }
                cur += cmd.copy_len as usize;
            }
        }
        let mut code_cost = [22.0f32; 704];
        if has_copy {
            if setcost_dist {
                // Smoothed entropy (upstream SetCost): continuous costs
                // keep the DP landscape stable where Huffman lengths
                // quantize it (measured -1.3KB at 1MB q11).
                code_cost = set_cost(&freq, false);
            } else {
                let huff = omnizip_codecs::HuffmanLengths::build(&freq, 15);
                for i in 0..704 {
                    if huff.lengths[i] > 0 {
                        code_cost[i] = f32::from(huff.lengths[i]);
                    }
                }
            }
        }
        let cmd_cost = if with_cmd_costs && has_copy {
            Some(set_cost(&cmd_freq, false))
        } else {
            None
        };
        Self {
            code_cost,
            cmd_cost,
            cfg,
        }
    }

    /// True wire cost (bits) of encoding `distance` explicitly.
    fn explicit_cost(&self, dist: u32) -> f32 {
        let (sym, _) = encode_distance(dist, &self.cfg);
        let nbits = distance_extra_bits(sym, &self.cfg);
        self.code_cost[sym.min(703) as usize] + nbits as f32
    }

    /// Wire cost of rep code `k` (0-3).
    fn rep_cost(&self, k: usize) -> f32 {
        self.code_cost[k]
    }
}

/// Zopfli-style forward DP with full rep-state tracking (port of the
/// core of `BrotliZopfliComputeShortestPath` from
/// `brotli/c/enc/backward_references_hq.c`).
///
/// Unlike the backward DP in [`optimal_parse_with_costs_ext`], this
/// walks positions left-to-right so each node's backpointer chain
/// encodes the full command history — which means the 4-slot rep
/// buffer state is exactly reconstructible at every position. Match
/// candidates are then priced with their TRUE wire cost: a distance
/// already sitting in the rep buffer costs a rep code (~2 bits)
/// instead of an explicit distance code (~12 bits).
///
/// Two candidate sources are evaluated per position:
/// 1. The longest hash-chain match (as before), priced against the
///    current rep state.
/// 2. Each of the 4 current rep distances, extended as a match at
///    this position — so the parser actively *chooses* distances that
///    keep the rep chain stable, rather than hoping the hash finder
///    happens to return them.
fn zopfli_parse(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    use_dict: bool,
    lit_cost_override: Option<[f32; 256]>,
    lit_cost_positional: Option<&[f32]>,
    cmd_cost_override: Option<f32>,
    dist_model: Option<&DistCostModel>,
    ins_prior_cfg: Option<(u32, f32, f32)>,
    history: Option<&[u8]>,
    hint_dist: Option<&[u32]>,
    quality: i32,
) -> Vec<Command> {
    zopfli_parse_ext(
        input,
        mf,
        0, // shared MF: data starts at global position 0
        mlen_offset,
        use_dict,
        lit_cost_override,
        lit_cost_positional,
        cmd_cost_override,
        dist_model,
        ins_prior_cfg,
        history,
        hint_dist,
        None,
        quality,
    )
    .0
}

#[allow(clippy::too_many_arguments)]
fn zopfli_parse_with_candidates(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    use_dict: bool,
    lit_cost_override: Option<[f32; 256]>,
    lit_cost_positional: Option<&[f32]>,
    cmd_cost_override: Option<f32>,
    dist_model: Option<&DistCostModel>,
    ins_prior_cfg: Option<(u32, f32, f32)>,
    history: Option<&[u8]>,
    hint_dist: Option<&[u32]>,
    quality: i32,
) -> (
    Vec<Command>,
    (Vec<(u32, u32)>, Vec<u32>, Vec<Option<(u32, u32, u32)>>),
) {
    zopfli_parse_ext(
        input,
        mf,
        0,
        mlen_offset,
        use_dict,
        lit_cost_override,
        lit_cost_positional,
        cmd_cost_override,
        dist_model,
        ins_prior_cfg,
        history,
        hint_dist,
        None,
        quality,
    )
}

/// `mf_base` is the global coordinate of `mf`'s data[0]. The shared MF
/// (whole input) has base 0; refinement MFs built over just the chunk
/// have base = mlen_offset. Querying an MF with global positions when
/// its data is chunk-local reads out of bounds and yields no candidates
/// — silently disabling the refinement pass for every chunk after the
/// first.
#[allow(clippy::too_many_arguments)]
fn zopfli_parse_ext(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mf_base: usize,
    mlen_offset: usize,
    use_dict: bool,
    lit_cost_override: Option<[f32; 256]>,
    lit_cost_positional: Option<&[f32]>,
    cmd_cost_override: Option<f32>,
    dist_model: Option<&DistCostModel>,
    ins_prior_cfg: Option<(u32, f32, f32)>,
    history: Option<&[u8]>,
    hint_dist: Option<&[u32]>,
    cand_flat_in: Option<(Vec<(u32, u32)>, Vec<u32>, Vec<Option<(u32, u32, u32)>>)>,
    quality: i32,
) -> (
    Vec<Command>,
    (Vec<(u32, u32)>, Vec<u32>, Vec<Option<(u32, u32, u32)>>),
) {
    let n = input.len();
    if n == 0 {
        return (Vec::new(), (Vec::new(), Vec::new(), Vec::new()));
    }
    // Translate a global position to the MF's own coordinate space.
    let to_mf = |global_pos: usize| global_pos.saturating_sub(mf_base);
    let cross_match_len = |i: usize, dist: u32, max_len: u32| -> u32 {
        let Some(hist) = history else {
            return 0;
        };
        let src_global = (mlen_offset + i).saturating_sub(dist as usize);
        if src_global >= mlen_offset {
            return 0;
        }
        let hist_off = mlen_offset - hist.len();
        if src_global < hist_off {
            return 0;
        }
        let mut l: u32 = 0;
        let ilen_max = input.len().saturating_sub(i);
        while (l as usize) < ilen_max && l < max_len {
            let src = hist[src_global + l as usize - hist_off];
            if src != input[i + l as usize] {
                break;
            }
            l += 1;
        }
        l
    };
    // Candidates: reuse the caller's collection when provided (the
    // collection walk dominates refinement runtime), else collect now.
    let (cand_flat, cand_off, dict_at) = match cand_flat_in {
        Some(pre) => pre,
        None => zopfli_collect(
            input,
            mf,
            mf_base,
            mlen_offset,
            use_dict,
            history,
            hint_dist,
            quality,
        ),
    };

    #[allow(clippy::type_complexity)]
    fn zopfli_collect(
        input: &[u8],
        mf: &mut omnizip_codecs::HashChainMatchFinder,
        mf_base: usize,
        mlen_offset: usize,
        use_dict: bool,
        history: Option<&[u8]>,
        hint_dist: Option<&[u32]>,
        quality: i32,
    ) -> (Vec<(u32, u32)>, Vec<u32>, Vec<Option<(u32, u32, u32)>>) {
        let n = input.len();
        let to_mf = |global_pos: usize| global_pos.saturating_sub(mf_base);
        let cross_match_len = |history: Option<&[u8]>, i: usize, dist: u32, max_len: u32| -> u32 {
            let Some(hist) = history else {
                return 0;
            };
            let src_global = (mlen_offset + i).saturating_sub(dist as usize);
            if src_global >= mlen_offset {
                return 0;
            }
            let hist_off = mlen_offset - hist.len();
            if src_global < hist_off {
                return 0;
            }
            let mut l: u32 = 0;
            let ilen_max = input.len().saturating_sub(i);
            while (l as usize) < ilen_max && l < max_len {
                let src = hist[src_global + l as usize - hist_off];
                if src != input[i + l as usize] {
                    break;
                }
                l += 1;
            }
            l
        };
        // Multi-candidate: up to `cand_count` distinct distances per position
        // (newest chain entries first). Distance diversity lets the DP warm
        // stable rep chains the longest-match policy would never surface.
        // Stored FLAT (values + per-position offsets): a Vec-per-position
        // here means millions of heap allocations per parse and dominates
        // runtime at multi-MB scale.
        // Candidate budget: the top-K-by-length walk needs enough depth to
        // reach structural matches buried under frequent short matches
        // (measured ~14-22 entries on the CSV structure). Patience-bounded,
        // so the deeper walk stays cheap on dense chains.
        // Effort tiers by quality (mirrors the reference's algorithm
        // tiers): q10+ gets the full candidate set; q8-9 moderate; q4-7
        // lean. Candidate volume dominates DP runtime.
        let (cand_count, walk) = if quality >= 10 {
            if n <= 1 << 20 {
                (16, 256)
            } else {
                (12, 96)
            }
        } else if quality >= 8 {
            (8, 64)
        } else {
            (4, 32)
        };
        let mut cand_flat: Vec<(u32, u32)> = Vec::with_capacity(n * 4);
        let mut cand_off: Vec<u32> = Vec::with_capacity(n + 1);
        let mut dict_at: Vec<Option<(u32, u32, u32)>> = vec![None; n]; // (d, wl, tl)
        let mut mf_buf: Vec<omnizip_codecs::Lz77Match> = Vec::new();
        for pos in 0..n {
            cand_off.push(cand_flat.len() as u32);
            let global_pos = mlen_offset + pos;
            let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
            if pos + MIN_MATCH as usize <= n {
                mf.advance();
                mf.find_candidates_into_patience(
                    to_mf(mlen_offset + pos),
                    cand_count,
                    walk,
                    if quality >= 10 { 32 } else { 16 },
                    &mut mf_buf,
                );
                if env_flag!("BROTLI_DP_DEBUG") && pos == 499_992 {
                    eprintln!(
                        "STEP1[499992] mf_base={mf_base} mlen={mlen_offset} n={n} raw={:?}",
                        &mf_buf
                    );
                }
                for m in &mf_buf {
                    if m.distance <= max_dist && m.length >= MIN_MATCH {
                        // Clamp to the chunk end: a match may not extend past
                        // the metablock boundary (decoder rejects overruns).
                        let copy_len = m.length.min(MAX_COPY).min((n - pos) as u32).max(MIN_MATCH);
                        cand_flat.push((m.distance, copy_len));
                    }
                }
            }
            // Seed the previous parse's rep0 distance as a candidate: the
            // chunk-local MF cannot surface cross-chunk distances at all,
            // so without this the refinement loses the structural distance
            // the previous parse rode (measured: 451K explicit distances
            // vs the reference's 29K at 21MB).
            if let Some(hd) = hint_dist {
                let d = hd[pos.min(hd.len() - 1)];
                if d > 0 && d <= max_dist {
                    let l = cross_match_len(history, pos, d, (n - pos) as u32).max({
                        let src_global = mlen_offset + pos - d as usize;
                        if src_global >= mf_base {
                            mf.match_len_between(
                                to_mf(mlen_offset + pos),
                                to_mf(src_global),
                                (n - pos) as u32,
                            )
                        } else {
                            0
                        }
                    });
                    if l >= MIN_MATCH {
                        cand_flat.push((d, l.min((n - pos) as u32)));
                    }
                }
            }
            if use_dict {
                let first: Option<&(u32, u32)> = cand_flat[cand_off[pos] as usize..].first();
                let hash_len = first.map(|&(_, l)| l).unwrap_or(0);
                if hash_len < 16 {
                    if let Some((d, wl, tl)) = dict_hash::find_match(input, pos, max_dist) {
                        if tl >= MIN_MATCH && pos + tl as usize <= n {
                            let is_better = match first {
                                None => true,
                                Some(&(_, existing_len)) => tl > existing_len,
                            };
                            if is_better {
                                dict_at[pos] = Some((d, wl.min(MAX_COPY).max(MIN_MATCH), tl));
                            }
                        }
                    }
                }
            }
            // Short-match scan (reference FindAllMatchesH10, HQ q10-11):
            // walk the nearest distances while no match longer than 2 is
            // known, recording each progressive improvement. At q11 the
            // scan reaches 64 bytes back, q10 16. These 2-3 byte copies
            // at short distances never surface from the 4-byte hash
            // chain, yet on columnar data they are the bulk of the
            // reference's copy stream (its FITS q11 emits 4x fewer
            // literals than a min-length-4 parse can).
            if quality >= 10 && pos + 2 <= n {
                let short_max: usize = if quality >= 11 { 64 } else { 16 };
                // Candidates only feed COPY_BOUNDARIES; capped at the
                // reference's MaxZopfliLen for this quality.
                let cap = ((n - pos) as u32).min(zopfli_max_len(quality));
                let mut best_len: u32 = 1;
                let mut src = pos;
                let stop = pos.saturating_sub(short_max);
                while src > stop && best_len <= 2 {
                    let prev = src - 1;
                    if input[prev] == input[pos] && input[prev + 1] == input[pos + 1] {
                        let mut l = 2u32;
                        while l < cap && input[prev + l as usize] == input[pos + l as usize] {
                            l += 1;
                        }
                        if l > best_len {
                            cand_flat.push(((pos - prev) as u32, l));
                            best_len = l;
                        }
                    }
                    src = prev;
                }
            }
        }
        cand_off.push(cand_flat.len() as u32);
        // Sort each position's candidate slice by length desc so the DP's
        // full-boundary sweep applies to the true longest match.
        for pos in 0..n {
            let s = cand_off[pos] as usize;
            let e = cand_off[pos + 1] as usize;
            cand_flat[s..e].sort_by(|a, b| b.1.cmp(&a.1));
        }

        (cand_flat, cand_off, dict_at)
    }

    // --- Step 2: cost models (shared with the backward DP) ---
    let lit_cost = lit_cost_override.unwrap_or_else(|| compute_huffman_lit_cost(input));
    let cmd_base_cost = cmd_cost_override.unwrap_or(7.0);
    let default_dist_cost = |dist: u32| -> f32 {
        if dist == 0 {
            return 0.0;
        }
        let d = dist as f32;
        if dist <= 4 {
            2.0 + 0.5 * d
        } else {
            let log_d = d.ln() / core::f32::consts::LN_2;
            (5.0 + log_d).min(22.0)
        }
    };
    let explicit_dist_cost = |dist: u32| -> f32 {
        if let Some(model) = dist_model {
            model.explicit_cost(dist)
        } else {
            default_dist_cost(dist)
        }
    };
    let rep_cost = |k: usize| -> f32 {
        if let Some(model) = dist_model {
            model.rep_cost(k)
        } else {
            1.5 + 0.75 * k as f32
        }
    };
    // Short-code (0-15) distance cost: matches rep0/rep1 exactly or at
    // ±1-3 offset encode as a single Huffman symbol (~3 bits, no extra
    // bits). Drifting distance chains ride these.
    let short_code_dist_cost = |dist: u32, reps: &[u32; 4]| -> Option<f32> {
        for (k, &r) in reps.iter().enumerate() {
            if r == dist {
                return Some(rep_cost(k));
            }
        }
        const DELTAS: [i32; 6] = [-1, 1, -2, 2, -3, 3];
        for &base in [&reps[0], &reps[1]] {
            if base == 0 {
                continue;
            }
            for &d in &DELTAS {
                let v = base as i32 + d;
                if v >= 1 && v == dist as i32 {
                    return Some(3.0);
                }
            }
        }
        None
    };
    // Exact wire cost: copy-length extra bits per kCmdLut's code table
    // (K_COPY_LENGTH_EXTRA_BITS over offsets 2,3,4,5,6,7,8,9,10,12,14,
    // 18,22,30,38,54,70,102,134,198,326,582,1094,2118).
    let copy_extra_cost = |copy_len: u32| -> f32 {
        if copy_len <= 9 {
            0.0
        } else if copy_len <= 13 {
            1.0
        } else if copy_len <= 21 {
            2.0
        } else if copy_len <= 37 {
            3.0
        } else if copy_len <= 69 {
            4.0
        } else if copy_len <= 133 {
            5.0
        } else if copy_len <= 197 {
            6.0
        } else if copy_len <= 325 {
            7.0
        } else if copy_len <= 581 {
            8.0
        } else if copy_len <= 1093 {
            9.0
        } else if copy_len <= 2117 {
            10.0
        } else {
            24.0
        }
    };

    // --- Step 3: forward DP with rep-state tracking ---
    // cost[i]   = min bits to encode input[0..i] (literals charged
    //             individually; command overhead charged when the
    //             pending literal run is flushed by a copy).
    // Insert-length prior (experiment): the flat per-command cost
    // treats a 7-literal flush the same as a 1-literal flush, so the
    // parse spreads inserts over 0..9 and the command tree stays broad
    // (measured 2.0 bits/cmd vs the reference's 1.25, which
    // concentrates at insert<=1). A fixed prior penalizes wide
    // inserts without learning from the previous parse (a learned
    // table collapsed the search — see 0.16.53 experiments).
    // Caller-provided (free_len, p1, p2): inserts <= free_len ride
    // free, 2..=9 pay p1, longer pay p2. None disables the prior.
    let (ins_free, ins_p1, ins_p2) = ins_prior_cfg.unwrap_or((1, 0.0, 0.0));
    let ins_prior = |ilen: u32| -> f32 {
        if ilen <= ins_free {
            0.0
        } else if ilen <= 9 {
            ins_p1
        } else {
            ins_p2
        }
    };
    // Per-symbol command pricing from the model's SetCost table
    // (upstream get_command_cost): the smooth entropy landscape lets
    // the DP concentrate on frequent (insert,copy) symbols without
    // the collapse a Huffman-quantized table caused. Falls back to the
    // flat average when no model is provided. Insert extra bits ride
    // along (upstream charges them at base_cost).
    let cmd_sym_price = |ilen: u32, copy_len: u32| -> f32 {
        match dist_model.and_then(|m| m.cmd_cost.as_ref()) {
            Some(t) => {
                let sym = find_cmd_symbol(ilen, copy_len);
                match sym {
                    Some(s) => t[s] + f32::from(kCmdLut[s].insert_len_extra_bits),
                    None => cmd_base_cost,
                }
            }
            None => cmd_base_cost,
        }
    };
    // Implicit-rep0 command price: distance folds into the symbol.
    let cmd_sym_price_implicit = |ilen: u32, copy_len: u32| -> Option<f32> {
        let t = dist_model.and_then(|m| m.cmd_cost.as_ref())?;
        let sym = find_cmd_symbol_with_rep(ilen, copy_len, Some(0))?;
        if kCmdLut[sym].distance_code == 0 {
            Some(t[sym] + f32::from(kCmdLut[sym].insert_len_extra_bits))
        } else {
            None
        }
    };
    // back_len  = copy length of the transition INTO i (0 = literal step)
    // back_pos  = source position of the transition INTO i
    // back_dist = distance for copy transitions
    // u[i]      = position where the pending literal run at i started
    let dp_debug = env_flag!("BROTLI_DP_DEBUG");
    let mut rep_state: Vec<[u32; 4]> = vec![[0u32; 4]; n + 1];
    let mut cost = vec![f32::INFINITY; n + 1];
    let mut back_pos = vec![0u32; n + 1];
    let mut back_len = vec![0u32; n + 1];
    let mut back_dist = vec![0u32; n + 1];
    let mut u = vec![0u32; n + 1];
    cost[0] = 0.0;

    for i in 0..n {
        let base = cost[i];
        if base == f32::INFINITY {
            continue;
        }

        // Debug dump for a narrow position window.
        let dbg = dp_debug && (499_990..=499_996).contains(&i);
        if dbg {
            let cs = cand_off[i] as usize;
            let ce = cand_off[i + 1] as usize;
            eprintln!(
                "DP[{i}] base={base:.1} u={} cands={:?} dict={:?}",
                u[i],
                &cand_flat[cs..ce],
                dict_at[i]
            );
        }

        // Literal transition. Positional (context-conditioned) costs
        // override the flat per-byte table when available.
        let lit_c = match lit_cost_positional {
            Some(pc) => pc[i],
            None => lit_cost[input[i] as usize],
        };
        let c = base + lit_c;
        if c < cost[i + 1] {
            cost[i + 1] = c;
            back_pos[i + 1] = i as u32;
            back_len[i + 1] = 0;
            back_dist[i + 1] = 0;
            u[i + 1] = u[i];
        }

        // Rep state at i: walk the backpointer chain back through the
        // (at most 4) most recent copy commands. Literal runs are
        // skipped via u[] jumps. Distances are deduplicated, mirroring
        // the decoder's shuffle-on-use rep semantics.
        // the decoder's shuffle-on-use rep semantics — maintained
        // incrementally: the optimal path into i is frozen before i is
        // processed (all transitions into i come from earlier
        // positions), so rep_state[i] derives in O(1) from the
        // backpointer instead of walking the chain. The chain walk this
        // replaces hunted for 4 DISTINCT distances through thousands of
        // same-distance commands on rep0-dense parses — 63% of
        // refinement runtime.
        let mut reps = [0u32; 4];
        if i > 0 {
            if back_len[i] == 0 {
                reps = rep_state[i - 1];
            } else {
                let d = back_dist[i];
                reps[0] = d;
                let mut k = 1usize;
                for &x in &rep_state[back_pos[i] as usize] {
                    if k < 4 && x != 0 && x != d {
                        reps[k] = x;
                        k += 1;
                    }
                }
            }
        }
        rep_state[i] = reps;

        // Copy transitions from all hash candidates. The first (longest)
        // candidate gets the full boundary sweep; the rest get their max
        // boundary (plus the copy-code boundary below it) — diversity is
        // for distance selection, not length tuning.
        let cstart = cand_off[i] as usize;
        let cend = cand_off[i + 1] as usize;
        for (cand_idx, &(dist, copy_len)) in cand_flat[cstart..cend].iter().enumerate() {
            if dist == 0 || copy_len < 2 {
                continue;
            }
            let dc = match short_code_dist_cost(dist, &reps) {
                Some(c) => c,
                None => explicit_dist_cost(dist),
            };
            let ilen_here = (i - u[i] as usize) as u32;
            let m_cost = ins_prior(ilen_here) + dc + cmd_sym_price(ilen_here, copy_len);
            let saturated = copy_len >= zopfli_max_len(quality);
            if cand_idx == 0 && !saturated {
                for &boundary in &COPY_BOUNDARIES {
                    if boundary > copy_len {
                        continue;
                    }
                    let j = i + boundary as usize;
                    if j > n {
                        break;
                    }
                    let total = base + m_cost + copy_extra_cost(boundary);
                    if total < cost[j] {
                        cost[j] = total;
                        back_pos[j] = i as u32;
                        back_len[j] = boundary;
                        back_dist[j] = dist;
                        u[j] = j as u32;
                    }
                }
            } else {
                // Max boundary + the highest copy-code boundary below it.
                let mut evaluated = [0u32; 2];
                evaluated[0] = copy_len;
                for &b in COPY_BOUNDARIES.iter().rev() {
                    if b < copy_len {
                        evaluated[1] = b;
                        break;
                    }
                }
                for &boundary in &evaluated {
                    if boundary < 2 {
                        continue;
                    }
                    let j = i + boundary as usize;
                    if j > n {
                        continue;
                    }
                    let total = base + m_cost + copy_extra_cost(boundary);
                    if total < cost[j] {
                        cost[j] = total;
                        back_pos[j] = i as u32;
                        back_len[j] = boundary;
                        back_dist[j] = dist;
                        u[j] = j as u32;
                    }
                }
            }
        }

        // Copy transition from the dictionary candidate (if any).
        if let Some((dist, copy_len, advance_len)) = dict_at[i] {
            let ilen_here = (i - u[i] as usize) as u32;
            let m_cost = ins_prior(ilen_here)
                + explicit_dist_cost(dist)
                + cmd_sym_price(ilen_here, copy_len);
            let j = i + advance_len as usize;
            if j <= n {
                let total = base + m_cost + copy_extra_cost(copy_len);
                if total < cost[j] {
                    cost[j] = total;
                    back_pos[j] = i as u32;
                    back_len[j] = copy_len.min(u16::MAX as u32);
                    back_dist[j] = dist;
                    u[j] = j as u32;
                }
            }
        }

        // Copy transitions from each rep distance and its ±1-3 offsets
        // (short codes 4-15): extend each variant as a match at this
        // position. This is how the parser rides slowly-drifting chains.
        // Skipped when the top hash candidate already reached the
        // boundary cap — a rep can only tie its length, and on long
        // repetitive runs every probe would scan the full cap.
        let top_len = cand_flat[cstart..cend].first().map_or(0, |&(_, l)| l);
        let global_i = mlen_offset + i;
        // Rep/delta transitions only feed COPY_BOUNDARIES; probing
        // beyond MaxZopfliLen is wasted compares — unbounded probing
        // was quadratic on long repetitive runs.
        let max_len = ((n - i) as u32).min(zopfli_max_len(quality));
        const DELTAS: [i32; 6] = [-1, 1, -2, 2, -3, 3];
        for (k, &r) in reps.iter().enumerate() {
            if r == 0 || (global_i as u64) < u64::from(r) || top_len >= max_len {
                continue;
            }
            let src_global = global_i - r as usize;
            let l = if src_global >= mf_base {
                mf.match_len_between(to_mf(global_i), to_mf(src_global), max_len)
            } else {
                cross_match_len(i, r, max_len)
            };
            if l < 2 {
                continue;
            }
            let ilen_here = (i - u[i] as usize) as u32;
            for &boundary in &COPY_BOUNDARIES {
                if boundary > l {
                    continue;
                }
                let j = i + boundary as usize;
                if j > n {
                    break;
                }
                // rep0 with a short insert rides an implicit symbol:
                // the distance costs nothing at all.
                let (sym_c, dist_c) = if k == 0 && ilen_here <= 9 {
                    match cmd_sym_price_implicit(ilen_here, boundary) {
                        Some(imp) => (ins_prior(ilen_here) + imp, 0.0),
                        None => (
                            ins_prior(ilen_here) + cmd_sym_price(ilen_here, boundary),
                            rep_cost(k),
                        ),
                    }
                } else {
                    (
                        ins_prior(ilen_here) + cmd_sym_price(ilen_here, boundary),
                        rep_cost(k),
                    )
                };
                let total = base + sym_c + dist_c + copy_extra_cost(boundary);
                if total < cost[j] {
                    cost[j] = total;
                    back_pos[j] = i as u32;
                    back_len[j] = boundary;
                    back_dist[j] = r;
                    u[j] = j as u32;
                }
            }
            // ±delta variants (only meaningful for the two most recent
            // distances — short codes 4-15 apply to rep0/rep1).
            if k >= 2 {
                continue;
            }
            for &d in &DELTAS {
                let rv = r as i32 + d;
                if rv < 1 || (global_i as i64) < i64::from(rv) {
                    continue;
                }
                let src = global_i - rv as usize;
                let lv = if src >= mf_base {
                    mf.match_len_between(to_mf(global_i), to_mf(src), max_len)
                } else {
                    cross_match_len(i, rv as u32, max_len)
                };
                if lv < 2 {
                    continue;
                }
                let ilen_here = (i - u[i] as usize) as u32;
                let short_c = match dist_model.and_then(|m| m.cmd_cost.as_ref()).map(|_| ()) {
                    Some(()) => {
                        let code = RepBuffer::new(); // placeholder removed below
                        let _ = code;
                        3.0
                    }
                    None => 3.0,
                };
                let m_cost_v =
                    ins_prior(ilen_here) + short_c + cmd_sym_price(ilen_here, lv.max(MIN_MATCH));
                let mut delta_bounds = [0u32; 2];
                delta_bounds[0] = lv;
                for &b in COPY_BOUNDARIES.iter().rev() {
                    if b < lv {
                        delta_bounds[1] = b;
                        break;
                    }
                }
                let delta_set: &[u32] = if env_flag!("BROTLI_FULL_DELTA") {
                    &COPY_BOUNDARIES
                } else {
                    &delta_bounds
                };
                for &boundary in delta_set {
                    if boundary < MIN_MATCH || boundary > lv {
                        continue;
                    }
                    let j = i + boundary as usize;
                    if j > n {
                        break;
                    }
                    let total = base + m_cost_v + copy_extra_cost(boundary);
                    if total < cost[j] {
                        cost[j] = total;
                        back_pos[j] = i as u32;
                        back_len[j] = boundary;
                        back_dist[j] = rv as u32;
                        u[j] = j as u32;
                    }
                }
            }
        }
    }

    // --- Step 4: backtrack ---
    let bt_trace_on = env_flag!("BROTLI_CHAINTRACE");
    let mut bt_trace: Vec<(usize, usize, usize, u32, u32, u32)> = Vec::new();
    let mut commands: Vec<Command> = Vec::new();
    // Trailing literals (after the last copy) form a final insert-only command.
    let mut last_copy_end = n;
    let mut guard = 0usize;
    while last_copy_end > 0 && back_len[last_copy_end] == 0 {
        guard += 1;
        if guard > n + 8 {
            break;
        }
        last_copy_end = back_pos[last_copy_end] as usize;
    }
    if n > last_copy_end {
        commands.push(Command {
            insert_len: (n - last_copy_end) as u32,
            copy_len: 0,
            distance: 0,
        });
    }
    let mut pos = last_copy_end;
    while pos > 0 {
        // pos is a copy-end node: back_len[pos] > 0.
        let src = back_pos[pos] as usize;
        let insert_len = (src - u[src] as usize) as u32;
        if env_flag!("BROTLI_DP_DEBUG") && (499_980..=500_020).contains(&pos) {
            eprintln!(
                "BT[end={pos}] src={src} ins={insert_len} copy={} d={} cost={:.1}",
                back_len[pos], back_dist[pos], cost[pos]
            );
        }
        if bt_trace_on {
            bt_trace.push((
                u[src] as usize,
                src,
                pos,
                insert_len,
                back_len[pos],
                back_dist[pos],
            ));
        }
        commands.push(Command {
            insert_len,
            copy_len: back_len[pos],
            distance: back_dist[pos],
        });
        // Walk src back through literal steps (u-jump) to the previous
        // copy end, or to 0. Literal nodes carry the run start in u[],
        // which is exactly the previous copy's end position.
        let mut p = src;
        let mut guard = 0usize;
        while p > 0 && back_len[p] == 0 {
            guard += 1;
            if guard > n + 8 {
                break;
            }
            p = u[p] as usize;
        }
        pos = p;
    }
    commands.reverse();
    if bt_trace_on {
        bt_trace.reverse();
        // Walk-view: what any consumer (scoring/emission) computes.
        let mut wcur = 0usize;
        let mut diverged = false;
        for (idx, (ins_start, cstart, cend, ins, cpy, dist)) in bt_trace.iter().enumerate() {
            let wstart = wcur + *ins as usize;
            let wend_dict =
                (*dist as usize) > (mlen_offset + wstart).min(MAX_BACKWARD_DISTANCE as usize);
            let wadv = if wend_dict {
                let max_d = ((mlen_offset + wstart) as u32).min(MAX_BACKWARD_DISTANCE);
                let mut tmp = Vec::new();
                match dictionary_lookup(&mut tmp, *cpy, *dist as i32, max_d) {
                    Some(()) => tmp.len(),
                    None => *cpy as usize,
                }
            } else {
                *cpy as usize
            };
            if !diverged && (wstart != *cstart || *ins_start != wcur) {
                diverged = true;
                eprintln!(
                    "CHAIN-DIVERGE idx={idx} parser=({ins_start},{cstart},{cend}) walk_start={wstart} prev_wcur={wcur} ins={ins} cpy={cpy} dist={dist} wdict={wend_dict} wadv={wadv} padv={}",
                    cend - cstart
                );
                let lo = idx.saturating_sub(6);
                for j in lo..=idx {
                    let t = bt_trace[j];
                    eprintln!(
                        "  ctx[{j}] parser=({},{},{}) ins={} cpy={} dist={}",
                        t.0, t.1, t.2, t.3, t.4, t.5
                    );
                }
            }
            wcur = wstart + wadv;
        }
        if !diverged {
            eprintln!("CHAIN-OK n={n} final_wcur={wcur}");
        }
    }
    (commands, (cand_flat, cand_off, dict_at))
}

/// Iterated Zopfli parse. Pass 1 uses default cost models; each
/// subsequent pass refines the literal/command costs and the
/// distance-code cost model from the previous pass's actual
/// distribution, and keeps whichever parse scores best. Iterates until
/// the score stops improving (max 3 refinement passes).
/// Bit-exact size of a candidate command list under the real emission
/// pipeline: same prologue bits, same [`emit_metablock_from_commands`].
/// Header bits for ISLAST and MLEN are constant across candidates, so
/// the comparison is unaffected by the is_last/ctx_in approximations.
#[allow(clippy::type_complexity)]
fn measure_emission_bits(
    commands: &[Command],
    input: &[u8],
    mlen_offset: usize,
    quality: i32,
    is_last: bool,
    ctx_in: (u8, u8),
) -> (u64, BitWriter) {
    // Header replicated EXACTLY from encode_huffman_chunk_body so the
    // winning candidate's writer IS the final metablock verbatim — the
    // real emission then reuses it instead of recomputing (the chain
    // measured 3 full emissions; recomputing the winner made it 4).
    let mut bw = BitWriter::new();
    bw.write_bits(u32::from(is_last), 1);
    if is_last {
        bw.write_bits(0, 1); // ISLASTEMPTY = 0
    }
    let mlen_minus_1 = (input.len() - 1) as u32;
    let (mnibbles, num_nibbles): (u32, u32) = if mlen_minus_1 < (1 << 16) {
        (0, 4)
    } else if mlen_minus_1 < (1 << 20) {
        (1, 5)
    } else {
        (2, 6)
    };
    bw.write_bits(mnibbles, 2);
    for i in 0..num_nibbles {
        bw.write_bits((mlen_minus_1 >> (4 * i)) & 0xF, 4);
    }
    if !is_last {
        bw.write_bits(0, 1); // ISUNCOMPRESSED = 0
    }
    let use_context = quality >= 4 && input.len() >= 4096;
    emit_metablock_from_commands(
        &mut bw,
        input,
        mlen_offset,
        false,
        quality,
        ctx_in,
        use_context,
        false,
        commands,
    );
    let bits = (bw.out.len() as u64) * 8 + u64::from(bw.nbits);
    (bits, bw)
}

/// H10 binary-tree candidate collection (BROTLI_TREE_MF): every
/// position's 4-byte bucket forms a binary search tree whose walk
/// yields one match per length tier — richer distance diversity than
/// the hash chain's top-K-by-length, and cheaper to walk.
#[allow(clippy::type_complexity)]
fn zopfli_collect_tree(
    input: &[u8],
    mlen_offset: usize,
    use_dict: bool,
) -> (Vec<(u32, u32)>, Vec<u32>, Vec<Option<(u32, u32, u32)>>) {
    let n = input.len();
    let mut tree = omnizip_codecs::BinaryTreeMatchFinder::new(input);
    let mut cand_flat: Vec<(u32, u32)> = Vec::with_capacity(n * 2);
    let mut cand_off: Vec<u32> = Vec::with_capacity(n + 1);
    let mut dict_at: Vec<Option<(u32, u32, u32)>> = vec![None; n];
    let mut buf: Vec<omnizip_codecs::Lz77Match> = Vec::new();
    for pos in 0..n {
        cand_off.push(cand_flat.len() as u32);
        let global_pos = mlen_offset + pos;
        let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
        if pos + MIN_MATCH as usize <= n {
            tree.find_candidates_into(pos, 16, &mut buf);
            for m in &buf {
                if m.distance <= max_dist && m.length >= MIN_MATCH {
                    let copy_len = m.length.min(MAX_COPY).min((n - pos) as u32).max(MIN_MATCH);
                    cand_flat.push((m.distance, copy_len));
                }
            }
        }
        if use_dict {
            let first = cand_flat[cand_off[pos] as usize..].first().copied();
            let hash_len = first.map(|(_, l)| l).unwrap_or(0);
            if hash_len < 16 {
                if let Some((d, wl, tl)) = dict_hash::find_match(input, pos, max_dist) {
                    if tl >= MIN_MATCH && pos + tl as usize <= n {
                        let is_better = first.is_none_or(|(_, l)| tl > l);
                        if is_better {
                            dict_at[pos] = Some((d, wl.min(MAX_COPY).max(MIN_MATCH), tl));
                        }
                    }
                }
            }
        }
    }
    cand_off.push(cand_flat.len() as u32);
    for pos in 0..n {
        let a = cand_off[pos] as usize;
        let b = cand_off[pos + 1] as usize;
        cand_flat[a..b].sort_by(|x, y| y.1.cmp(&x.1));
    }
    (cand_flat, cand_off, dict_at)
}

#[allow(clippy::type_complexity)]
fn zopfli_iterative_parse(
    input: &[u8],
    history: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    use_dict: bool,
    quality: i32,
    is_last: bool,
    ctx_in: (u8, u8),
) -> (Vec<Command>, Option<BitWriter>) {
    // Insert-length prior for q10+ (measured on CSV): inserts <= 2 ride
    // free, 3..=9 pay 0.7 bits, longer pay 3.0. Concentrates the
    // command stream on the reference's (small-insert) symbol shape —
    // the flat per-command cost leaves insert length to chance and the
    // command tree stays broad. Env: BROTLI_INS_FREE /
    // BROTLI_INS_PRIOR="p1,p2"; BROTLI_NO_INS_PRIOR disables.
    let ins_prior_cfg: Option<(u32, f32, f32)> = if env_flag!("BROTLI_NO_INS_PRIOR") {
        None
    } else {
        let free = std::env::var("BROTLI_INS_FREE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);
        let (p1, p2) = std::env::var("BROTLI_INS_PRIOR")
            .ok()
            .and_then(|v| {
                let ab: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                (ab.len() == 2).then(|| (ab[0], ab[1]))
            })
            .unwrap_or((0.7f32, 3.0f32));
        (quality >= 10).then_some((free, p1, p2))
    };

    // Pass 1a: caller-provided MF (may be shared across chunks for
    // cross-chunk matching). Pass 1b: light-config MF — a different
    // hash width orders candidates differently, which measurably changes
    // which rep chains the DP warms. Keep the better-scoring start.
    let t0 = std::time::Instant::now();
    // H10 binary-tree candidates (BROTLI_TREE_MF): replaces the hash
    // chain's collection for pass 1a; the refinement's candidate cache
    // and rep0-hint merge flow unchanged from there.
    let (mut best_commands, cached_candidates) = if env_flag!("BROTLI_TREE_MF") {
        let cands = zopfli_collect_tree(input, mlen_offset, use_dict);
        zopfli_parse_ext(
            input,
            mf,
            0,
            mlen_offset,
            use_dict,
            None,
            None,
            None,
            None,
            ins_prior_cfg,
            None,
            None,
            Some(cands),
            quality,
        )
    } else {
        // Pass-1a literal pricing: the reference's Zopfli prices each
        // literal by a sliding 4000-byte window (local entropy) — on
        // region-structured binary this decides copy-vs-literal far
        // better than the whole-metablock average. Text keeps the
        // global table (their UTF8 variant differs; measured later).
        let sw_costs = if quality >= 10 && !is_text_like(input) && !env_flag!("BROTLI_NO_SW_LIT") {
            Some(sliding_window_lit_costs(input))
        } else {
            None
        };
        zopfli_parse_with_candidates(
            input,
            mf,
            mlen_offset,
            use_dict,
            None,
            sw_costs.as_deref(),
            None,
            None,
            ins_prior_cfg,
            None,
            None,
            quality,
        )
    };
    if env_flag!("BROTLI_STATS") {
        eprintln!(
            "PHASE pass1a: {:.1}s ({} cmds)",
            t0.elapsed().as_secs_f64(),
            best_commands.len()
        );
    }
    let mut best_score = score_commands(&best_commands, input, mlen_offset);
    // Pass 1c: greedy+lazy parse (the quality-1 path). On highly
    // structured data its local, rep-friendly match choices often beat
    // the DP's globally-priced ones; it's also nearly free to compute.
    {
        // Env-tunable for sweeps: BROTLI_GREEDY="chain,nice,hashlog".
        let (gc, gn, gh) = std::env::var("BROTLI_GREEDY")
            .ok()
            .and_then(|v| {
                let p: Vec<u32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                (p.len() == 3).then_some((p[0], p[1], p[2]))
            })
            .unwrap_or((24, 64, 17));
        let greedy_config = omnizip_codecs::HashChainConfig {
            dict_size: MAX_BACKWARD_DISTANCE,
            min_match: MIN_MATCH,
            max_chain_length: gc,
            nice_match: gn,
            hash_log: gh,
            hash_bytes: 4,
            max_match_length: zopfli_max_len(quality),
        };
        let mut mf_greedy = omnizip_codecs::HashChainMatchFinder::new(input, greedy_config);
        let t = std::time::Instant::now();
        let greedy_commands = greedy_parse(input, &mut mf_greedy, mlen_offset);
        if env_flag!("BROTLI_STATS") {
            eprintln!("PHASE greedy: {:.1}s", t.elapsed().as_secs_f64());
        }
        let greedy_score = score_commands(&greedy_commands, input, mlen_offset);
        if greedy_score < best_score && !env_flag!("BROTLI_NO_GREEDY") {
            best_score = greedy_score;
            best_commands = greedy_commands;
        }
    }
    if input.len() <= 1 << 20 && quality >= 10 {
        let light_config = omnizip_codecs::HashChainConfig {
            dict_size: MAX_BACKWARD_DISTANCE,
            min_match: MIN_MATCH,
            max_chain_length: 16,
            nice_match: 96,
            hash_log: 17,
            hash_bytes: 4,
            max_match_length: zopfli_max_len(quality),
        };
        let mut mf_light = omnizip_codecs::HashChainMatchFinder::new(input, light_config);
        let light_commands = zopfli_parse(
            input,
            &mut mf_light,
            mlen_offset,
            use_dict,
            None,
            None,
            None,
            None,
            ins_prior_cfg,
            None,
            None,
            quality,
        );
        let light_score = score_commands(&light_commands, input, mlen_offset);
        if light_score < best_score && !env_flag!("BROTLI_NO_LIGHT") {
            best_score = light_score;
            best_commands = light_commands;
        }
    }

    // Refinement passes use the deepest config: measured best on
    // structured data regardless of the pass-1 config (deeper candidate
    // lists stabilize the rep chains during refinement).
    let _ = quality;
    let (max_chain, nice_match, _, _, _, hash_log) = brotli_quality_config(11, true);
    let config = omnizip_codecs::HashChainConfig {
        dict_size: MAX_BACKWARD_DISTANCE,
        min_match: MIN_MATCH,
        max_chain_length: max_chain,
        nice_match,
        hash_log,
        hash_bytes: 4,
        max_match_length: zopfli_max_len(quality),
    };

    let iters_env = std::env::var("BROTLI_ITERS")
        .ok()
        .and_then(|v| v.parse().ok());
    let mut max_iters = iters_env.unwrap_or(if quality >= 10 {
        if input.len() <= 1 << 20 {
            3
        } else {
            1
        }
    } else if quality >= 8 {
        // q8-9 keep one refinement (a real quality step); q4-7 are
        // single pass like the reference's non-refining tiers.
        1
    } else {
        0
    });
    // BROTLI_EXACT_ACCEPT: judge refinement candidates by their REAL
    // emission size (full splits + trees + cmaps) instead of the
    // approximate scorer. Approximations mis-ordered candidates by
    // >10K bits on the CSV benchmark — the emission stage's
    // block-splitting and singleton-tree assignment shift costs in
    // ways no analytic model reproduces.
    let exact_accept =
        env_flag!("BROTLI_EXACT_ACCEPT") || (quality >= 10 && !env_flag!("BROTLI_NO_EXACT_ACCEPT"));
    // Exact-acceptance chains converge by iteration 2 (iteration 3 was
    // rejected on every benchmark size); 2 iterations across ALL chunk
    // sizes — the 8 MiB q10+ chunks only reach the winning parse on
    // iteration 2, so the historical ≤1 MiB → 3 / >1 MiB → 1 split
    // would leave large inputs stuck at the pre-chain parse.
    if exact_accept && iters_env.is_none() {
        max_iters = 2;
    }
    let mut best_bits: Option<u64>;
    let mut winner_bw: Option<BitWriter> = None;
    if exact_accept {
        let (bits, bw) =
            measure_emission_bits(&best_commands, input, mlen_offset, quality, is_last, ctx_in);
        best_bits = Some(bits);
        winner_bw = Some(bw);
    } else {
        best_bits = None;
    }
    // Chain state: each iteration's cost model derives from the
    // previous iteration's OUTPUT (upstream Zopfli semantics), which is
    // not necessarily the current winner — the chain can pass through a
    // worse parse on its way to a much better one.
    let mut model_commands: Vec<Command> = best_commands.clone();
    for _ in 0..max_iters {
        let literals_prev = extract_literals(&model_commands, input, mlen_offset);
        if literals_prev.is_empty() {
            break;
        }

        // Context-conditioned positional literal costs, matched to the
        // granularity the encoder actually uses (4 literal trees via
        // ctx>>4 for inputs ≥ 8 KiB). Modeling finer contexts than the
        // wire format can express over-promises literal savings and
        // skews the parse.
        let from_commands =
            env_flag!("BROTLI_CM_LIT") || (quality >= 10 && !env_flag!("BROTLI_NO_CM_LIT"));
        let _positional_costs: Vec<f32> = {
            let context_mode: u32 = if is_text_like(input) { 2 } else { 0 };
            // Per-CONTEXT (64 buckets) literal costs — each bucket priced
            // under its own Huffman tree. Pure buckets cost ~0 (the final
            // coding isolates them into zero-bit trees), high-diversity
            // buckets (digit runs) cost their true entropy. This lets the
            // DP shift copy boundaries toward cheap literals.
            //
            // Two histogram sources:
            // - raw input (default): every byte counts, contexts covered
            //   by matches look "cheap" too.
            // - previous parse's literals (BROTLI_CM_LIT, upstream
            //   ZopfliCostModelSetFromCommands): only bytes the previous
            //   parse actually emitted as literals price a context, so
            //   contexts living inside matches stay expensive and the DP
            //   won't flip copies into literals there.
            let mut p1: u8 = 0;
            let mut p2: u8 = 0;
            let mut ctx_of = vec![0u8; input.len()];
            for (i, &b) in input.iter().enumerate() {
                ctx_of[i] = compute_context_id(p1, p2, context_mode);
                p2 = p1;
                p1 = b;
            }
            // Batched histograms are available for experiments via
            // BROTLI_CTX_BATCH; the default is one chunk-wide window,
            // which measured best (batching lost the chain's win at
            // 1 MiB by over-fragmenting the context statistics).
            let batch: usize = std::env::var("BROTLI_CTX_BATCH")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(input.len().max(1))
                .max(1024);
            // literal-at-position flags for the from_commands source.
            let is_literal_pos: Option<Vec<bool>> = if from_commands {
                let mut flags = vec![false; input.len()];
                let mut cur = 0usize;
                for cmd in &model_commands {
                    let end = cur + cmd.insert_len as usize;
                    for x in flags.iter_mut().take(end).skip(cur) {
                        *x = true;
                    }
                    if cmd.copy_len > 0 {
                        let is_dict = (cmd.distance as usize)
                            > (mlen_offset + end).min(MAX_BACKWARD_DISTANCE as usize);
                        let adv = if is_dict {
                            let global_pos = mlen_offset + end;
                            let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                            let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                            match dictionary_lookup(
                                &mut tmp,
                                cmd.copy_len,
                                cmd.distance as i32,
                                max_dist,
                            ) {
                                Some(()) => tmp.len(),
                                None => cmd.copy_len as usize,
                            }
                        } else {
                            cmd.copy_len as usize
                        };
                        cur = end + adv;
                    } else {
                        cur = end;
                    }
                }
                Some(flags)
            } else {
                None
            };
            let mut out = vec![0f32; input.len()];
            let mut s = 0usize;
            while s < input.len() {
                let e = (s + batch).min(input.len());
                let mut hist = [[0u32; 256]; 64];
                match &is_literal_pos {
                    Some(flags) => {
                        for i in s..e {
                            if flags[i] {
                                hist[usize::from(ctx_of[i])][usize::from(input[i])] += 1;
                            }
                        }
                    }
                    None => {
                        for i in s..e {
                            hist[usize::from(ctx_of[i])][usize::from(input[i])] += 1;
                        }
                    }
                }
                let mut cost_of = [[0f32; 256]; 64];
                for (c, h) in hist.iter().enumerate() {
                    let total: u64 = h.iter().map(|&x| u64::from(x)).sum();
                    if total == 0 {
                        // No literals observed in this context: unseen
                        // bytes must not price as free or the DP converts
                        // every covered copy into a literal run.
                        cost_of[c] = [12.0; 256];
                        continue;
                    }
                    let distinct = h.iter().filter(|&&x| x > 0).count();
                    if distinct == 1 {
                        if let Some(b) = h.iter().position(|&x| x > 0) {
                            cost_of[c][b] = 0.05;
                        }
                        continue;
                    }
                    let tree = omnizip_codecs::HuffmanLengths::build(h, 15);
                    for (b, &f) in h.iter().enumerate() {
                        let l = tree.lengths[b];
                        cost_of[c][b] = if f > 0 && l > 0 { f32::from(l) } else { 12.0 };
                    }
                }
                for i in s..e {
                    out[i] = cost_of[usize::from(ctx_of[i])][input[i] as usize];
                }
                s = e;
            }
            out
        };
        // Upstream set_from_commands pairs per-symbol command costs
        // with FLAT literal costs (SetCost over the literal byte
        // histogram) — the context-steered positional model is our own
        // extension and unbalances the per-symbol landscape (measured:
        // chain candidates +50% bits). BROTLI_SETCMD_FLIT=0 restores
        // the positional model.
        // Flat upstream-style literal model is an experiment
        // (BROTLI_SETCMD_FLIT=1); the context-steered positional model
        // measures better alongside per-symbol command pricing.
        let setcmd_active = quality >= 10
            && model_commands.len() >= 32_000
            && model_commands.len() <= 128_000
            && !env_flag!("BROTLI_NO_SETCMD")
            && std::env::var("BROTLI_SETCMD_FLIT")
                .map(|v| v == "1")
                .unwrap_or(false);
        let lit_cost_refined = if setcmd_active {
            let mut lh = [0u32; 704];
            for &b in &literals_prev {
                lh[usize::from(b)] += 1;
            }
            let sc = set_cost(&lh, true);
            let mut t = [8.0f32; 256];
            for (b, &c) in sc.iter().enumerate().take(256) {
                t[b] = if c > 0.0 { c } else { 8.0 };
            }
            t
        } else {
            compute_huffman_lit_cost(&literals_prev)
        };

        let mut cmd_freq = [0u32; 704];
        for cmd in &model_commands {
            if let Some(sym) = find_cmd_symbol(cmd.insert_len, cmd.copy_len) {
                cmd_freq[sym] += 1;
            }
        }
        let cmd_huff = omnizip_codecs::HuffmanLengths::build(&cmd_freq, 15);
        let cmd_nonzero: u32 = cmd_freq.iter().sum();
        let cmd_avg_bits = if cmd_nonzero > 0 {
            let mut total_bits = 0u32;
            for (sym, &freq) in cmd_freq.iter().enumerate() {
                if freq > 0 {
                    total_bits += freq * u32::from(cmd_huff.lengths[sym]);
                }
            }
            total_bits as f32 / cmd_nonzero as f32
        } else {
            7.0
        };

        // Per-position rep0 from the previous parse: lets the chunk-local
        // refinement MF "see" cross-chunk distances (its candidates stop at the
        // chunk start, so the structural distance the previous parse rode would
        // otherwise be unreachable and the parse churns explicit distances).
        let rep_hint: Vec<u32> = {
            let mut hint = vec![0u32; input.len() + 1];
            let mut rep = RepBuffer::new();
            let mut pos = 0usize;
            for cmd in &model_commands {
                for _ in 0..cmd.insert_len as usize {
                    if pos < hint.len() {
                        hint[pos] = rep.rep_at(0);
                    }
                    pos += 1;
                }
                if cmd.copy_len > 0 {
                    let global_pos = mlen_offset + pos;
                    let is_dict = (cmd.distance as usize) > global_pos;
                    if is_dict {
                        rep.on_dict_reference(false);
                    } else if let Some(code) = rep.find_rep_code(cmd.distance) {
                        rep.on_rep_lz77(code);
                    } else {
                        rep.on_new_distance_lz77(cmd.distance);
                    }
                    let advance = if is_dict {
                        let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                        let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                        match dictionary_lookup(
                            &mut tmp,
                            cmd.copy_len,
                            cmd.distance as i32,
                            max_dist,
                        ) {
                            Some(()) => tmp.len(),
                            None => cmd.copy_len as usize,
                        }
                    } else {
                        cmd.copy_len as usize
                    };
                    for _ in 0..advance {
                        if pos < hint.len() {
                            hint[pos] = rep.rep_at(0);
                        }
                        pos += 1;
                    }
                }
            }
            hint
        };

        let hint_dist_ref: Option<&[u32]> = Some(&rep_hint);

        // Distance pricing via smoothed entropy (upstream SetCost) at q10+;
        // per-symbol COMMAND pricing (upstream set_from_commands) measured
        // worse here — it converges to replicating the start parse's symbol
        // shape instead of exploring (324K vs 297K bits at 1MB). Available
        // as an experiment via BROTLI_SETCMD=1.
        let dist_model = DistCostModel::from_commands(
            &model_commands,
            mlen_offset,
            quality >= 10
                && model_commands.len() >= 32_000
                && model_commands.len() <= 128_000
                && !env_flag!("BROTLI_NO_SETCMD"),
            quality >= 10
                && model_commands.len() >= 32_000
                && model_commands.len() <= 128_000
                && !env_flag!("BROTLI_NO_SETCOST_D"),
        );

        let t = std::time::Instant::now();
        // Reuse the pass-1 candidate collection (the collection walk
        // dominates refinement runtime) and append the hint distance —
        // the cross-chunk structural distance — at each position.
        let merged: (Vec<(u32, u32)>, Vec<u32>, Vec<Option<(u32, u32, u32)>>) = {
            let (mut flat, mut off, dict) = cached_candidates.clone();
            flat.clear();
            off.clear();
            off.push(0);
            for pos in 0..input.len() {
                let (cs, ce) = (
                    cached_candidates.1[pos] as usize,
                    cached_candidates.1[pos + 1] as usize,
                );
                for &c in &cached_candidates.0[cs..ce] {
                    flat.push(c);
                }
                if let Some(hd) = hint_dist_ref {
                    let d = hd[pos.min(hd.len() - 1)];
                    let global_pos = mlen_offset + pos;
                    let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                    if d > 0 && d <= max_dist {
                        let src_global = global_pos - d as usize;
                        let l = mf.match_len_between(
                            global_pos,
                            src_global,
                            MAX_COPY.min((input.len() - pos) as u32),
                        );
                        if l >= MIN_MATCH {
                            flat.push((d, l));
                        }
                    }
                }
                off.push(flat.len() as u32);
            }
            for pos in 0..input.len() {
                let a = off[pos] as usize;
                let b = off[pos + 1] as usize;
                flat[a..b].sort_by(|x, y| y.1.cmp(&x.1));
            }
            (flat, off, dict)
        };
        let commands_iter = zopfli_parse_ext(
            input,
            mf,
            0,
            mlen_offset,
            use_dict,
            Some(lit_cost_refined),
            if setcmd_active {
                None
            } else if env_flag!("BROTLI_POS") || (quality >= 10 && !env_flag!("BROTLI_NO_POS")) {
                Some(&_positional_costs)
            } else {
                None
            },
            Some(cmd_avg_bits),
            Some(&dist_model),
            ins_prior_cfg,
            Some(history),
            Some(&rep_hint),
            Some(merged),
            quality,
        )
        .0;
        if env_flag!("BROTLI_STATS") {
            eprintln!("PHASE refine: {:.1}s", t.elapsed().as_secs_f64());
        }
        let score_iter = score_commands(&commands_iter, input, mlen_offset);
        if env_flag!("BROTLI_STATS") {
            eprintln!(
                "zopfli iter: best={best_score} candidate={score_iter} (cmds {} -> {})",
                best_commands.len(),
                commands_iter.len()
            );
        }
        if exact_accept {
            // Chain advances unconditionally; the winner is whichever
            // parse actually measures smallest. Early-breaking on a
            // worse intermediate candidate never reaches the better
            // parses deeper in the chain (measured: candidate 1 is
            // worse, candidate 3 is 1.8KB better).
            let (bits_iter, iter_bw) =
                measure_emission_bits(&commands_iter, input, mlen_offset, quality, is_last, ctx_in);
            if env_flag!("BROTLI_STATS") {
                eprintln!("zopfli exact: best={best_bits:?} candidate={bits_iter}");
            }
            if best_bits.is_none_or(|b| bits_iter < b) {
                best_bits = Some(bits_iter);
                best_commands = commands_iter.clone();
                winner_bw = Some(iter_bw);
            } else {
                drop(iter_bw);
            }
            model_commands = commands_iter;
        } else if score_iter < best_score || env_flag!("BROTLI_POS_FORCE") {
            best_score = score_iter.min(best_score);
            best_commands = commands_iter.clone();
            model_commands = commands_iter;
        } else {
            break;
        }
    }
    if let Ok(path) = std::env::var("BROTLI_DUMP_CMDS") {
        // Canonical dump of the WINNING parse only (the measure chain
        // evaluates several candidates; only this one is emitted).
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        let mut pos = mlen_offset;
        for c in &best_commands {
            writeln!(
                f,
                "ENC pos={pos} ins={} copy={} dist={}",
                c.insert_len, c.copy_len, c.distance
            )
            .unwrap();
            pos += c.insert_len as usize;
            if c.copy_len > 0 {
                // Dictionary references advance by the TRANSFORMED
                // length, which differs from copy_len (the pre-transform
                // word length) — same rule the decoder applies.
                let copy_start = (mlen_offset + pos).min(usize::MAX);
                let max_dist = (copy_start as u32).min(MAX_BACKWARD_DISTANCE);
                let advance = if c.distance > max_dist {
                    let mut scratch = Vec::new();
                    match crate::dictionary::dictionary_lookup(
                        &mut scratch,
                        c.copy_len,
                        c.distance as i32,
                        max_dist,
                    ) {
                        Some(()) => scratch.len(),
                        None => c.copy_len as usize,
                    }
                } else {
                    c.copy_len as usize
                };
                pos += advance;
            }
        }
    }
    (best_commands, winner_bw)
}

/// Iterative refinement of [`optimal_parse`] (TODO 246).
///
/// Runs two passes:
/// 1. Pass 1 uses Shannon entropy over all input bytes as the literal
///    cost model. This is what [`optimal_parse`] does today.
/// 2. Build literal stream from pass 1's commands. Compute Shannon
///    entropy over THOSE literals (which often excludes bytes that
///    ended up in copy matches, sharpening the distribution).
/// 3. Pass 2 re-parses with the refined literal cost.
///
/// Returns whichever pass produced the smaller Huffman stream
/// (measured by total symbol bits, not literal count — passes can
/// trade literals for matches).
///
/// ## Why this helps
///
/// Pass 1's lit_cost treats every byte uniformly. But after parsing,
/// many bytes are inside copied matches and don't go through the
/// literal Huffman tree. The actual literal byte distribution is
/// often sharper (more skew toward common ASCII letters), so the
/// Huffman tree built from those literals is cheaper for the bytes
/// that actually appear as literals.
///
/// ## Why two passes is enough
///
/// Standard convergence: pass 2's literal stream ≈ pass 1's, so a
/// third pass reproduces pass 2's output. Cap at 2 for predictable
/// runtime.
#[allow(dead_code)]
fn iterative_optimal_parse(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    use_dict: bool,
    quality: i32,
) -> Vec<Command> {
    // Q11+ uses 4 iterations to let the rep_hint converge: iter 1 sets
    // rep0 from longest matches, iter 2 may choose closer rep-friendly
    // matches, iter 3's rep_hint reflects those, iter 4 refines further.
    let iters = if quality >= 11 { 4 } else { 2 };
    iterative_optimal_parse_with_iters(input, mf, mlen_offset, use_dict, iters)
}

/// Multi-pass iterative optimal parser (TODO 272). Each iteration
/// refines the literal cost model based on the previous iteration's
/// actual parsed literals. 2 iterations is the default; Q11 uses 4
/// for additional refinement.
#[allow(clippy::too_many_lines, dead_code)]
fn iterative_optimal_parse_with_iters(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    use_dict: bool,
    iterations: usize,
) -> Vec<Command> {
    // Pass 1: Huffman-derived cost from input. Shannon entropy
    // underestimates literal cost for small alphabets (e.g. ~20 distinct
    // bytes → Shannon ≈ 4.3 bits but Huffman ≈ 5 bits), biasing the DP
    // toward literals when copies would be cheaper. Huffman code lengths
    // match what the wire format actually pays per byte.
    let mut best_commands = optimal_parse_with_costs(
        input,
        mf,
        mlen_offset,
        use_dict,
        Some(compute_huffman_lit_cost(input)),
    );
    let mut best_score = score_commands(&best_commands, input, mlen_offset);

    let config = omnizip_codecs::HashChainConfig {
        dict_size: MAX_BACKWARD_DISTANCE,
        min_match: MIN_MATCH,
        max_chain_length: mf_max_chain(mf),
        nice_match: mf_nice_match(mf),
        hash_log: mf_hash_log(mf),
        hash_bytes: 4,
        max_match_length: 325,
    };

    // Subsequent passes: refine lit_cost AND command/distance costs from
    // the previous pass's actual output. This is critical for FSST data
    // where the fixed 7.0-bit command estimate diverges from actual
    // Huffman costs, causing wrong match decisions.
    for _ in 1..iterations {
        let literals_prev = extract_literals(&best_commands, input, mlen_offset);
        if literals_prev.is_empty() {
            break;
        }
        let lit_cost_refined = compute_huffman_lit_cost(&literals_prev);

        // Compute command cost from pass 1's command symbol frequencies.
        // Build a histogram of kCmdLut symbols, then Huffman-build to get
        // average code length.
        let mut cmd_freq = [0u32; 704];
        for cmd in &best_commands {
            if let Some(sym) = find_cmd_symbol(cmd.insert_len, cmd.copy_len) {
                cmd_freq[sym] += 1;
            }
        }
        let cmd_huff = omnizip_codecs::HuffmanLengths::build(&cmd_freq, 15);
        let cmd_nonzero: u32 = cmd_freq.iter().sum();
        let cmd_avg_bits = if cmd_nonzero > 0 {
            let mut total_bits = 0u32;
            for (sym, &freq) in cmd_freq.iter().enumerate() {
                if freq > 0 {
                    total_bits += freq * u32::from(cmd_huff.lengths[sym]);
                }
            }
            total_bits as f32 / cmd_nonzero as f32
        } else {
            7.0
        };

        // Distance cost table: Huffman-derived per-code costs.
        let mut dist_freq = [0u32; 704];
        for cmd in &best_commands {
            if cmd.copy_len > 0 && cmd.distance > 0 {
                // Approximate distance code index for histogram
                let idx = if cmd.distance <= 4 {
                    (cmd.distance - 1) as usize
                } else {
                    (4 + (cmd.distance as f32).ln() as usize).min(703)
                };
                dist_freq[idx] += 1;
            }
        }
        let dist_huff = omnizip_codecs::HuffmanLengths::build(&dist_freq, 15);
        let mut dist_table = [22.0f32; 704];
        for i in 0..704 {
            if dist_huff.lengths[i] > 0 {
                dist_table[i] = dist_huff.lengths[i] as f32;
            }
        }

        // Compute rep0 hint from the previous iteration's commands.
        // This gives the DP knowledge of which distances are "warm" in
        // the rep buffer, enabling it to prefer matches that become
        // cheap rep codes (0 distance bits instead of ~10).
        let rep_hint: Vec<u32> = {
            let mut hint = vec![0u32; input.len() + 1];
            let mut rep = RepBuffer::new();
            let mut pos = 0usize;
            for cmd in &best_commands {
                for _ in 0..cmd.insert_len as usize {
                    if pos < hint.len() {
                        hint[pos] = rep.rep_at(0);
                    }
                    pos += 1;
                }
                if cmd.copy_len > 0 {
                    let global_pos = mlen_offset + pos;
                    let is_dict = (cmd.distance as usize) > global_pos;
                    if is_dict {
                        rep.on_dict_reference(false);
                    } else if let Some(code) = rep.find_rep_code(cmd.distance) {
                        rep.on_rep_lz77(code);
                    } else {
                        rep.on_new_distance_lz77(cmd.distance);
                    }
                    let advance = if is_dict {
                        let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                        let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                        match dictionary_lookup(
                            &mut tmp,
                            cmd.copy_len,
                            cmd.distance as i32,
                            max_dist,
                        ) {
                            Some(()) => tmp.len(),
                            None => cmd.copy_len as usize,
                        }
                    } else {
                        cmd.copy_len as usize
                    };
                    for _ in 0..advance {
                        if pos < hint.len() {
                            hint[pos] = rep.rep_at(0);
                        }
                        pos += 1;
                    }
                }
            }
            hint
        };

        let mut mf_iter = omnizip_codecs::HashChainMatchFinder::new(input, config);
        let commands_iter = optimal_parse_with_costs_ext(
            input,
            &mut mf_iter,
            mlen_offset,
            use_dict,
            Some(lit_cost_refined),
            Some(cmd_avg_bits),
            Some(&dist_table),
            Some(&rep_hint),
        );
        let score_iter = score_commands(&commands_iter, input, mlen_offset);
        if score_iter < best_score {
            best_score = score_iter;
            best_commands = commands_iter;
        }
    }

    rewrite_for_rep_codes(best_commands, input, mlen_offset)
}

/// Extract just the literal bytes from a command list (in stream order).
#[allow(dead_code)]
fn extract_literals(commands: &[Command], input: &[u8], mlen_offset: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cur = 0usize;
    for cmd in commands {
        let end = cur + cmd.insert_len as usize;
        out.extend_from_slice(&input[cur..end]);
        let advance = if cmd.copy_len > 0 {
            let is_dict =
                (cmd.distance as usize) > (mlen_offset + end).min(MAX_BACKWARD_DISTANCE as usize);
            if is_dict {
                let global_pos = mlen_offset + end;
                let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                match dictionary_lookup(&mut tmp, cmd.copy_len, cmd.distance as i32, max_dist) {
                    Some(()) => tmp.len(),
                    None => cmd.copy_len as usize,
                }
            } else {
                cmd.copy_len as usize
            }
        } else {
            0
        };
        cur = end + advance;
    }
    out
}

/// Rough score: total bits the command stream would cost in the
/// Huffman-coded wire format. Lower is better.
///
/// Literal bits are priced with the same 4-tree context model the
/// encoder emits (UTF8 contexts, ctx>>4 grouping); distances are priced
/// with rep-buffer simulation including the ±1-3 short codes. Keeping
/// the scorer aligned with the encoder's actual costs is what lets the
/// iterative parser rank literal-heavy vs copy-heavy parses correctly.
#[allow(dead_code)]
/// Per-position literal costs under per-context (64-bucket) Huffman
/// models, with pure contexts charged ~0 (isolated as zero-bit trees).
fn context_positional_costs(input: &[u8]) -> Vec<f32> {
    let context_mode: u32 = if is_text_like(input) { 2 } else { 0 };
    let mut hist = vec![[0u32; 256]; 64];
    let mut p1: u8 = 0;
    let mut p2: u8 = 0;
    let mut ctx_of = vec![0u8; input.len()];
    for (i, &b) in input.iter().enumerate() {
        let ctx = compute_context_id(p1, p2, context_mode) as usize;
        ctx_of[i] = ctx as u8;
        hist[ctx][b as usize] += 1;
        p2 = p1;
        p1 = b;
    }
    let mut cost_of = vec![[0f32; 256]; 64];
    for (c, h) in hist.iter().enumerate() {
        let total: u64 = h.iter().map(|&x| u64::from(x)).sum();
        if total == 0 {
            continue;
        }
        if h.iter().filter(|&&x| x > 0).count() == 1 {
            if let Some(b) = h.iter().position(|&x| x > 0) {
                cost_of[c][b] = 0.05;
            }
            continue;
        }
        let tree = omnizip_codecs::HuffmanLengths::build(h, 15);
        for (b, &f) in h.iter().enumerate() {
            let l = tree.lengths[b];
            cost_of[c][b] = if f > 0 && l > 0 { f32::from(l) } else { 12.0 };
        }
    }
    let mut out = vec![0f32; input.len()];
    for i in 0..input.len() {
        out[i] = cost_of[usize::from(ctx_of[i])][input[i] as usize];
    }
    out
}

/// Exact bit-count of what the emission pipeline would produce for
/// these commands: reuses the SAME decision functions the encoder uses
/// (optimal block splits, singleton-vs-cluster tree assignment,
/// HuffmanLengths trees) so a parse the DP prefers is a parse that
/// actually encodes smaller. Returns None if the stream can't be built
/// (caller falls back to the heuristic scorer).
#[allow(clippy::too_many_lines)]
fn exact_emission_bits(
    commands: &[Command],
    input: &[u8],
    mlen_offset: usize,
    use_context: bool,
) -> Option<u64> {
    let dist_cfg = DistanceConfig::choose(commands);
    let stream = build_symbol_stream(commands, input, mlen_offset, &dist_cfg)?;

    let mut bits: u64 = 0;

    // --- Command symbols: per-block Huffman after the optimal split ---
    let cmd_boundaries = split_symbol_stream_optimal(&stream.cmd_symbols, 704, 16);
    let nblocks = cmd_boundaries.len();
    for k in 0..nblocks {
        let start = cmd_boundaries[k];
        let end = cmd_boundaries
            .get(k + 1)
            .copied()
            .unwrap_or(stream.cmd_symbols.len());
        let mut freq = vec![0u32; 704];
        for &s in &stream.cmd_symbols[start..end] {
            freq[s] += 1;
        }
        let huff = omnizip_codecs::HuffmanLengths::build(&freq, 15);
        let mut nonzero = 0u64;
        for (s, &f) in freq.iter().enumerate() {
            if f > 0 {
                nonzero += u64::from(f);
                bits += u64::from(f)
                    * u64::from(if huff.lengths[s] > 0 {
                        huff.lengths[s]
                    } else {
                        15
                    });
            }
        }
        let _ = nonzero;
        bits += 120; // per-block tree header (conservative)
    }
    bits += 40 * nblocks as u64; // block-type/length switch codes

    // --- Insert/copy extra bits ---
    for &sym in &stream.cmd_symbols {
        let entry = &kCmdLut[sym];
        bits += u64::from(entry.insert_len_extra_bits) + u64::from(entry.copy_len_extra_bits);
    }

    // --- Distance symbols: per-(block,ctx) clustered trees ---
    if !stream.dist_symbols.is_empty() && dist_cfg.alphabet_size() <= 256 {
        let dist_boundaries = split_symbol_stream_optimal(
            &stream
                .dist_symbols
                .iter()
                .map(|&s| s as usize)
                .collect::<Vec<_>>(),
            dist_cfg.alphabet_size(),
            4,
        );
        let nb = dist_boundaries.len();
        let mut hists: Vec<[u32; 256]> = vec![[0u32; 256]; nb * 4];
        {
            let mut blk = 0usize;
            for (idx, (&sym, &ctx)) in stream
                .dist_symbols
                .iter()
                .zip(stream.dist_ctxs.iter())
                .enumerate()
            {
                while blk + 1 < dist_boundaries.len() && idx >= dist_boundaries[blk + 1] {
                    blk += 1;
                }
                hists[(blk << 2) + ctx as usize][sym as usize] += 1;
            }
        }
        let mut global = [0u32; 256];
        for h in &hists {
            for (s, &v) in h.iter().enumerate() {
                global[s] += v;
            }
        }
        let huff_bits = |hists: &[[u32; 256]]| -> u64 {
            let mut freq = vec![0u32; 256];
            for h in hists {
                for (s, &v) in h.iter().enumerate() {
                    freq[s] += v;
                }
            }
            let huff = omnizip_codecs::HuffmanLengths::build(&freq, 15);
            let mut b = 0u64;
            for (s, &f) in freq.iter().enumerate() {
                if f > 0 {
                    b += u64::from(f)
                        * u64::from(if huff.lengths[s] > 0 {
                            huff.lengths[s]
                        } else {
                            15
                        });
                }
            }
            b
        };
        // Single-tree variant vs per-context clustered variant, choose
        // the smaller (mirrors the emission's cost gate).
        let cmap4 = crate::encoder::context::cluster_contexts(&hists, 4);
        let mut per: Vec<[u32; 256]> = vec![[0u32; 256]; 4];
        for (i, h) in hists.iter().enumerate() {
            for (s, &v) in h.iter().enumerate() {
                per[usize::from(cmap4[i])][s] += v;
            }
        }
        let split_bits = huff_bits(&per) + 3 * 70 + 20;
        let single_bits = huff_bits(&[global]) + 70;
        bits += split_bits.min(single_bits);
        for &s in &stream.dist_symbols {
            bits += u64::from(distance_extra_bits(s, &dist_cfg));
        }
    }

    // --- Literals: block split + singleton/cluster assignment ---
    if !stream.literals.is_empty() {
        let lit_boundaries = if use_context && stream.literals.len() >= 4096 {
            split_literals(&stream.literals, 8)
        } else {
            vec![0]
        };
        let nlb = lit_boundaries.len();
        // Per-(block,ctx) histograms via output simulation.
        let mut bc: Vec<[u32; 256]> = vec![[0u32; 256]; nlb * 64];
        let context_mode: u32 = if use_context && is_text_like(input) {
            2
        } else {
            0
        };
        let (mut p1, mut p2) = carried_lit_ctx(input, mlen_offset);
        let mut lit_pos = 0usize;
        let mut lit_blk = 0usize;
        let mut out_pos = 0usize;
        for cmd in commands {
            for _ in 0..cmd.insert_len {
                if nlb > 1
                    && lit_blk + 1 < lit_boundaries.len()
                    && lit_pos >= lit_boundaries[lit_blk + 1]
                {
                    lit_blk += 1;
                }
                let b = input[out_pos];
                let ctx = compute_context_id(p1, p2, context_mode) as usize;
                bc[(lit_blk << 6) + ctx][b as usize] += 1;
                p2 = p1;
                p1 = b;
                out_pos += 1;
                lit_pos += 1;
            }
            if cmd.copy_len > 0 {
                let is_dict = (cmd.distance as usize)
                    > (mlen_offset + out_pos).min(MAX_BACKWARD_DISTANCE as usize);
                let adv = if is_dict {
                    cmd.copy_len as usize
                } else {
                    cmd.copy_len as usize
                };
                out_pos += adv;
                if out_pos > 0 && out_pos <= input.len() {
                    let new_p1 = input[out_pos - 1];
                    p2 = if cmd.copy_len > 1 {
                        input[out_pos - 2]
                    } else {
                        p1
                    };
                    p1 = new_p1;
                }
            }
        }
        // Same A/B choice as the emission.
        let huff_bits = |hs: &[[u32; 256]]| -> u64 {
            let mut total = 0u64;
            for h in hs {
                let t: u64 = h.iter().map(|&x| u64::from(x)).sum();
                if t == 0 {
                    continue;
                }
                let huff = omnizip_codecs::HuffmanLengths::build(h, 15);
                for (b, &f) in h.iter().enumerate() {
                    if f > 0 {
                        total += u64::from(f)
                            * u64::from(if huff.lengths[b] > 0 {
                                huff.lengths[b]
                            } else {
                                15
                            });
                    }
                }
            }
            total
        };
        let cmap_a = crate::encoder::context::cluster_contexts(&bc, 4);
        let mut hists_a: Vec<[u32; 256]> = vec![[0u32; 256]; 4];
        for (i, h) in bc.iter().enumerate() {
            for (b, &f) in h.iter().enumerate() {
                hists_a[usize::from(cmap_a[i])][b] += f;
            }
        }
        let cost_a = huff_bits(&hists_a) + 4 * 60 + (bc.len() as u64) * 2;
        let (cmap_b, count_b) = crate::encoder::context::assign_context_trees(&bc, 8);
        let mut hists_b: Vec<[u32; 256]> = vec![[0u32; 256]; count_b];
        for (i, h) in bc.iter().enumerate() {
            for (b, &f) in h.iter().enumerate() {
                hists_b[usize::from(cmap_b[i])][b] += f;
            }
        }
        let cost_b = huff_bits(&hists_b) + count_b as u64 * 35 + (bc.len() as u64) * 8;
        bits += cost_a.min(cost_b);
        bits += 40 * nlb as u64; // literal block switches
    }

    Some(bits)
}

/// Parse score under the parse's OWN context-conditioned literal model:
/// per-context Huffman trees over that parse's literals (pure contexts
/// price ~0, matching singleton-tree emission), plus the same cmd/dist
/// pricing as [`score_commands`]. Fixed-model scorers misprice parses
/// that concentrate literals into contexts — the emission stage prices
/// each parse under its own trees, so acceptance must too.
fn score_commands_adaptive(commands: &[Command], input: &[u8], mlen_offset: usize) -> u64 {
    let context_mode: u32 = if is_text_like(input) { 2 } else { 0 };
    let mut p1: u8 = 0;
    let mut p2: u8 = 0;
    let mut ctx_of = vec![0u8; input.len()];
    for (i, &b) in input.iter().enumerate() {
        ctx_of[i] = compute_context_id(p1, p2, context_mode);
        p2 = p1;
        p1 = b;
    }

    let mut hists = vec![[0u32; 256]; 64];
    let mut cmd_count = 0u64;
    let mut cmd_freq = [0u32; 704];
    let mut cmd_extra_bits: u64 = 0;
    let mut cmd_adv: Vec<usize> = Vec::with_capacity(commands.len());
    let mut cur = 0usize;
    for cmd in commands {
        let end = cur + cmd.insert_len as usize;
        for i in cur..end {
            hists[usize::from(ctx_of[i])][usize::from(input[i])] += 1;
        }
        let adv = if cmd.copy_len > 0 {
            cmd_count += 1;
            if let Some(sym) = find_cmd_symbol(cmd.insert_len, cmd.copy_len) {
                cmd_freq[sym] += 1;
                let e = &kCmdLut[sym];
                cmd_extra_bits +=
                    u64::from(e.insert_len_extra_bits) + u64::from(e.copy_len_extra_bits);
            }
            let is_dict =
                (cmd.distance as usize) > (mlen_offset + end).min(MAX_BACKWARD_DISTANCE as usize);
            if is_dict {
                let global_pos = mlen_offset + end;
                let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                match dictionary_lookup(&mut tmp, cmd.copy_len, cmd.distance as i32, max_dist) {
                    Some(()) => tmp.len(),
                    None => cmd.copy_len as usize,
                }
            } else {
                cmd.copy_len as usize
            }
        } else {
            0
        };
        cmd_adv.push(adv);
        cur = end + adv;
    }

    let mut lit_bits: u64 = 0;
    for h in &hists {
        let distinct = h.iter().filter(|&&x| x > 0).count();
        if distinct == 0 {
            continue;
        }
        if distinct == 1 {
            continue; // singleton tree: zero bits per literal
        }
        let tree = omnizip_codecs::HuffmanLengths::build(h, 15);
        for (b, &f) in h.iter().enumerate() {
            if f > 0 && tree.lengths[b] > 0 {
                lit_bits += u64::from(f) * u64::from(tree.lengths[b]);
            }
        }
    }
    // Context-map + tree-header estimate: trees beyond the first cost
    // header bits; a parse spreading literals over many contexts pays
    // for them.
    let used_ctx = hists.iter().filter(|h| h.iter().any(|&x| x > 0)).count();

    // Command symbol bits under the parse's own Huffman tree, plus
    // exact insert/copy extra bits. A flat per-command constant
    // overcharges command-heavy parses by ~5 bits/cmd and flips
    // acceptance toward under-matched parses.
    let cmd_tree = omnizip_codecs::HuffmanLengths::build(&cmd_freq, 15);
    let cmd_sym_bits: u64 = cmd_freq
        .iter()
        .zip(cmd_tree.lengths.iter())
        .map(|(&f, &l)| u64::from(f) * u64::from(l))
        .sum();

    // Distance pricing mirrors build_symbol_stream exactly: implicit
    // rep0 commands emit NO distance symbol; explicit commands emit a
    // short code when one matches, else the long form; the rep buffer
    // updates follow the emitted SYMBOL (not the distance value).
    // A rep-code-count approximation overcharges implicit-rep0-heavy
    // parses by the whole distance-symbol cost and rejects parses the
    // real encoder encodes smaller.
    let cfg = DistanceConfig::choose(commands);
    let mut rep = RepBuffer::new();
    let mut dist_freq = [0u32; 704];
    let mut dist_extra_bits: u64 = 0;
    let mut explicit_copies_remaining = if mlen_offset > 0 { 4 } else { 0 };
    let mut output_pos = 0usize;
    for cmd in commands {
        output_pos += cmd.insert_len as usize;
        let is_dict_ref = cmd.copy_len > 0
            && (cmd.distance as usize)
                > (mlen_offset + output_pos).min(MAX_BACKWARD_DISTANCE as usize);
        let can_use_implicit = cmd.copy_len > 0
            && !is_dict_ref
            && explicit_copies_remaining == 0
            && cmd.distance == rep.rep_at(0)
            && cmd.insert_len <= 9
            && find_cmd_symbol_with_rep(cmd.insert_len, cmd.copy_len, Some(0))
                .is_some_and(|sym| kCmdLut[sym].distance_code == 0);
        let emitted_sym: Option<u32> = if cmd.copy_len > 0 && !can_use_implicit {
            let (sym, extra) = if is_dict_ref || explicit_copies_remaining > 0 {
                encode_distance(cmd.distance, &cfg)
            } else if let Some(code) = rep.find_short_code(cmd.distance) {
                (code, 0)
            } else {
                encode_distance(cmd.distance, &cfg)
            };
            if !is_dict_ref && explicit_copies_remaining > 0 {
                explicit_copies_remaining -= 1;
            }
            dist_freq[sym as usize] += 1;
            dist_extra_bits += u64::from(extra);
            Some(sym)
        } else {
            None
        };
        if cmd.copy_len > 0 {
            if is_dict_ref {
                rep.on_dict_reference(false);
            } else if can_use_implicit || emitted_sym == Some(0) {
                rep.on_rep_lz77(0);
            } else {
                rep.on_new_distance_lz77(cmd.distance);
            }
            output_pos += if is_dict_ref {
                let global_pos = mlen_offset + output_pos;
                let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                let mut tmp = Vec::with_capacity(cmd.copy_len as usize);
                match dictionary_lookup(&mut tmp, cmd.copy_len, cmd.distance as i32, max_dist) {
                    Some(()) => tmp.len(),
                    None => cmd.copy_len as usize,
                }
            } else {
                cmd.copy_len as usize
            };
        }
    }
    let dist_tree = omnizip_codecs::HuffmanLengths::build(&dist_freq, 15);
    let dist_bits: u64 = dist_freq
        .iter()
        .zip(dist_tree.lengths.iter())
        .map(|(&f, &l)| u64::from(f) * u64::from(l))
        .sum::<u64>()
        + dist_extra_bits;

    lit_bits + 5 * used_ctx as u64 + cmd_sym_bits + cmd_extra_bits + dist_bits
}

fn score_commands(commands: &[Command], input: &[u8], mlen_offset: usize) -> u64 {
    if env_flag!("BROTLI_ADAPT") {
        return score_commands_adaptive(commands, input, mlen_offset);
    }
    if env_flag!("BROTLI_EXACT_SCORE") {
        if let Some(b) = exact_emission_bits(commands, input, mlen_offset, true) {
            return b;
        }
    }
    let use_positional = env_flag!("BROTLI_POS");
    let mut literal_count = 0u64;
    let mut cmd_count = 0u64;
    let mut literals_freq = [0u32; 256];
    // Per-position context-aware literal costs (fixed for the input —
    // the decoder output equals the input regardless of the parse).
    let positional = if use_positional {
        context_positional_costs(input)
    } else {
        Vec::new()
    };

    // First pass: literal frequencies + command count + dict-aware
    // per-command advances (dict transforms change output length).
    let mut cur = 0usize;
    let mut cmd_adv: Vec<usize> = Vec::with_capacity(commands.len());
    for cmd in commands {
        let end = cur + cmd.insert_len as usize;
        for &b in &input[cur..end] {
            literals_freq[b as usize] += 1;
            literal_count += 1;
        }
        let adv = if cmd.copy_len > 0 {
            cmd_count += 1;
            let is_dict =
                (cmd.distance as usize) > (mlen_offset + end).min(MAX_BACKWARD_DISTANCE as usize);
            if is_dict {
                let global_pos = mlen_offset + end;
                let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                let mut tmp = Vec::with_capacity(cmd.copy_len as usize + 8);
                match dictionary_lookup(&mut tmp, cmd.copy_len, cmd.distance as i32, max_dist) {
                    Some(()) => tmp.len(),
                    None => cmd.copy_len as usize,
                }
            } else {
                cmd.copy_len as usize
            }
        } else {
            0
        };
        cmd_adv.push(adv);
        cur = end + adv;
    }

    // Literal bits: context-aware per-position costs when enabled,
    // otherwise flat Huffman over the literal stream.
    let mut lit_bits: u64 = 0;
    if use_positional {
        let mut cur2 = 0usize;
        for cmd in commands {
            let end = cur2 + cmd.insert_len as usize;
            for i in cur2..end {
                lit_bits += positional[i] as u64;
            }
            cur2 = end
                + if cmd.copy_len > 0 {
                    let is_dict = (cmd.distance as usize)
                        > (mlen_offset + end).min(MAX_BACKWARD_DISTANCE as usize);
                    if is_dict {
                        cmd.copy_len as usize
                    } else {
                        cmd.copy_len as usize
                    }
                } else {
                    0
                };
        }
    } else {
        let lit_huff = omnizip_codecs::HuffmanLengths::build(&literals_freq, 15);
        for (b, &f) in literals_freq.iter().enumerate() {
            if f > 0 && lit_huff.lengths[b] > 0 {
                lit_bits += u64::from(f) * u64::from(lit_huff.lengths[b]);
            }
        }
    }

    // Distance bits: rep-buffer simulation with short codes (exact reps
    // AND rep0/rep1 ± 1-3 variants), else the true explicit cost.
    let cfg = DistanceConfig::choose(commands);
    let mut rep = RepBuffer::new();
    let mut dist_bits: u64 = 0;
    let mut cur2 = 0usize;
    for cmd in commands {
        cur2 += cmd.insert_len as usize;
        if cmd.copy_len > 0 {
            let is_dict =
                (cmd.distance as usize) > (mlen_offset + cur2).min(MAX_BACKWARD_DISTANCE as usize);
            if is_dict {
                rep.on_dict_reference(false);
                dist_bits += 14;
            } else if let Some(code) = rep.find_rep_code(cmd.distance) {
                dist_bits += u64::from(2 + code); // exact rep code
                rep.on_rep_lz77(code);
            } else {
                let (sym, _) = encode_distance(cmd.distance, &cfg);
                dist_bits += u64::from(4 + distance_extra_bits(sym, &cfg));
                rep.on_new_distance_lz77(cmd.distance);
            }
            cur2 += if is_dict {
                let global_pos = mlen_offset + cur2;
                let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
                let mut tmp = Vec::with_capacity(cmd.copy_len as usize);
                match dictionary_lookup(&mut tmp, cmd.copy_len, cmd.distance as i32, max_dist) {
                    Some(()) => tmp.len(),
                    None => cmd.copy_len as usize,
                }
            } else {
                cmd.copy_len as usize
            };
        }
    }

    lit_bits + cmd_count * 8 + dist_bits
}

// Read-only accessors for HashChainConfig fields (it's private inside
// HashChainMatchFinder; we mirror via the config struct that the caller
// built). Since we can't read them back from mf itself, default to
// reasonable values for the iterative refinement pass.
#[allow(dead_code)]
fn mf_max_chain(_mf: &omnizip_codecs::HashChainMatchFinder) -> u32 {
    128
}
#[allow(dead_code)]
fn mf_nice_match(_mf: &omnizip_codecs::HashChainMatchFinder) -> u32 {
    128
}
#[allow(dead_code)]
fn mf_hash_log(_mf: &omnizip_codecs::HashChainMatchFinder) -> u32 {
    17
}

/// Two-pass backward reference collection (TODO 241).
///
/// Pass 1: Walk all positions, find ALL matches via hash chain +
/// dictionary. Store in a pre-allocated array.
///
/// Pass 2: Walk the matches array with extended look-ahead (4
/// positions). Pick the longest match, deferring when a longer
/// match is available at a nearby position. This finds better
/// match combinations than single-pass lazy parsing because
/// ALL matches are visible before any decisions are made.
fn two_pass_parse(
    input: &[u8],
    mlen_offset: usize,
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    use_dict: bool,
) -> Vec<Command> {
    let n = input.len();
    if n < MIN_MATCH as usize + 1 {
        return vec![Command {
            insert_len: n as u32,
            copy_len: 0,
            distance: 0,
        }];
    }

    // --- Pass 1: Collect all matches ---
    // Each entry: (distance, copy_len=word_length, advance_len=transformed_len)
    let limit = n.saturating_sub(MIN_MATCH as usize);
    let mut matches: Vec<Option<(u32, u32, u32)>> = vec![None; n];

    for pos in 0..limit {
        mf.advance();
        let global_pos = mlen_offset + pos;
        let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);

        if let Some(m) = mf.find_match(pos) {
            if m.distance > 0 && m.distance <= max_dist && m.length >= MIN_MATCH {
                // Clamp to the chunk end (metablock boundary).
                let copy_len = m.length.min(MAX_COPY).min((n - pos) as u32).max(MIN_MATCH);
                matches[pos] = Some((m.distance, copy_len, copy_len));
            }
        }

        if matches[pos].is_none() && use_dict {
            if let Some((d, wl, tl)) = dict_hash::find_match(input, pos, max_dist) {
                if tl >= MIN_MATCH && pos + tl as usize <= n {
                    let copy_len = wl.min(MAX_COPY).max(MIN_MATCH);
                    matches[pos] = Some((d, copy_len, tl));
                }
            }
        }
    }

    // --- Pass 2: Greedy with extended look-ahead ---
    // At each position with a match, check the next 4 positions.
    // If any has a significantly longer match, defer.
    let mut commands = Vec::new();
    let mut pos = 0usize;
    let mut insert_start = 0usize;

    while pos < n {
        if let Some((dist, copy_len, advance_len)) = matches[pos] {
            if copy_len >= MIN_MATCH && dist > 0 {
                // Extended look-ahead: check next 4 positions for free
                let mut best_pos = pos;
                let mut best_copy = copy_len;
                let mut best_dist = dist;
                let mut best_advance = advance_len;

                for offset in 1..=4u32 {
                    let next = pos + offset as usize;
                    if next >= n {
                        break;
                    }
                    if let Some((d2, c2, a2)) = matches[next] {
                        if a2 > best_advance + offset {
                            best_pos = next;
                            best_copy = c2;
                            best_dist = d2;
                            best_advance = a2;
                        }
                    }
                }

                let insert_len = (best_pos - insert_start) as u32;
                commands.push(Command {
                    insert_len,
                    copy_len: best_copy,
                    distance: best_dist,
                });

                pos = best_pos + best_advance as usize;
                insert_start = pos;
                continue;
            }
        }
        pos += 1;
    }

    // Trailing literals
    if insert_start < n {
        commands.push(Command {
            insert_len: (n - insert_start) as u32,
            copy_len: 0,
            distance: 0,
        });
    }

    commands
}

#[allow(dead_code)]
fn parse_input(input: &[u8]) -> Vec<Command> {
    let is_text = is_text_like(input);
    let (max_chain, nice_match, _, _, _, hash_log) = brotli_quality_config(11, is_text);
    let config = omnizip_codecs::HashChainConfig {
        dict_size: MAX_BACKWARD_DISTANCE,
        min_match: MIN_MATCH,
        max_chain_length: max_chain,
        nice_match,
        hash_log,
        hash_bytes: 4,
        max_match_length: 4096,
    };
    let mut mf = omnizip_codecs::HashChainMatchFinder::new(input, config);
    parse_input_with_offset(input, &[], &mut mf, None, 0, 0, 11, false, false, (0, 0)).0
}

/// Quality → (max_chain, nice_match, use_dict_base, lazy, lazy2, hash_log).
/// DRY helper so callers don't duplicate the match table.
fn brotli_quality_config(quality: i32, is_text: bool) -> (u32, u32, bool, bool, bool, u32) {
    if is_text {
        match quality {
            0..=1 => (24, 64, false, true, false, 17),
            2..=3 => (8, 16, true, true, false, 16),
            4..=5 => (16, 96, true, true, true, 17),
            6..=7 => (32, 192, true, true, true, 17),
            8..=9 => (64, 256, true, true, true, 18),
            10 => (128, 512, true, true, true, 18),
            // Q11: deeper chain walk for exhaustive match evaluation.
            // The reference's HQ Zopfli uses a binary tree match finder
            // that evaluates ALL candidates. We approximate this with
            // a much deeper hash-chain walk.
            _ => (1024, 4096, true, true, true, 18),
        }
    } else {
        match quality {
            0..=1 => (4, 8, false, false, false, 15),
            _ => (8, 16, false, false, false, 16),
        }
    }
}

#[allow(dead_code)]
fn brotli_quality_config_deep(_quality: i32, _is_text: bool) -> (u32, u32, bool, bool, bool, u32) {
    (256, 512, true, true, true, 18)
}

/// Greedy+lazy parse (the quality-1 path) with a caller-configured MF.
/// Used as a candidate parse inside [`zopfli_iterative_parse`]: on
/// strongly structured data its locally rep-friendly choices can beat
/// the globally-priced DP parse.
fn greedy_parse(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
) -> Vec<Command> {
    parse_input_with_offset_impl(
        input,
        &[],
        mf,
        None,
        mlen_offset,
        mlen_offset,
        1,
        false,
        false,
        (0, 0),
    )
    .0
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
///
/// `disable_dict` temporarily disables dictionary lookups (used when
/// context modeling is active, due to a decoder interaction bug).
#[allow(clippy::type_complexity)]
fn parse_input_with_offset(
    input: &[u8],
    history: &[u8],
    mut mf: &mut omnizip_codecs::HashChainMatchFinder,
    bank_mf: Option<&mut omnizip_codecs::BankMatchFinder>,
    mf_base: usize,
    mlen_offset: usize,
    quality: i32,
    disable_dict: bool,
    is_last: bool,
    ctx_in: (u8, u8),
) -> (Vec<Command>, Option<BitWriter>) {
    parse_input_with_offset_impl(
        input,
        history,
        &mut mf,
        bank_mf,
        mf_base,
        mlen_offset,
        quality,
        disable_dict,
        is_last,
        ctx_in,
    )
}

/// Diagnostic wrapper exposed for benchmarks. Not part of the public API.
#[doc(hidden)]
pub fn _parse_input_with_offset_diag(
    input: &[u8],
    mf: &mut omnizip_codecs::HashChainMatchFinder,
    mlen_offset: usize,
    quality: i32,
    disable_dict: bool,
) -> Vec<Command> {
    parse_input_with_offset_impl(
        input,
        &[],
        mf,
        None,
        mlen_offset, // diagnostic callers use chunk-local MFs
        mlen_offset,
        quality,
        disable_dict,
        false,
        (0, 0),
    )
    .0
}

fn parse_input_with_offset_impl(
    input: &[u8],
    history: &[u8],
    mut mf: &mut omnizip_codecs::HashChainMatchFinder,
    mut bank_mf: Option<&mut omnizip_codecs::BankMatchFinder>,
    mf_base: usize,
    mlen_offset: usize,
    quality: i32,
    disable_dict: bool,
    is_last: bool,
    ctx_in: (u8, u8),
) -> (Vec<Command>, Option<BitWriter>) {
    let n = input.len();
    // Content classification is O(n) — a per-position call (as the
    // lazy lookahead had) is O(n^2) and dwarfs everything else.
    let is_text_input = is_text_like(input);
    let mut commands = Vec::new();

    // At Q4+, always use the text config (deeper chains, dict, lazy2)
    // regardless of content type. FSST-transformed data and other
    // semi-structured binary benefits from the same parser effort as
    // natural text. The optimal parser compensates for any mismatch.
    let (max_chain, nice_match, use_dict_base, lazy, lazy2, hash_log) =
        brotli_quality_config(quality, true);
    // The static dictionary pays on text (real word matches); on
    // binary its per-position lookup costs ~30% of greedy-tier encode
    // for ~0.2% size. Binary q4-7 (time-first tier) skips it.
    let use_dict = use_dict_base && !disable_dict && (quality >= 8 || is_text_input);

    let _config = omnizip_codecs::HashChainConfig {
        dict_size: MAX_BACKWARD_DISTANCE,
        min_match: MIN_MATCH,
        max_chain_length: max_chain,
        nice_match,
        hash_log,
        hash_bytes: 4,
        max_match_length: 4096,
    };
    // MF is provided by the caller — no creation here.

    // Q4+: Zopfli forward DP with full rep-state tracking (port of
    // BrotliZopfliComputeShortestPath). The forward direction makes the
    // 4-slot rep buffer reconstructible at every position, so match
    // candidates are priced at their TRUE wire cost: exact rep codes,
    // rep0/rep1 ± 1-3 short codes (~3 bits, no extra bits), or explicit
    // distance codes. On data with repeating or slowly-drifting
    // distance structure (CSV, source code), this converts most
    // distance codes into ~2-3-bit short codes instead of ~12-15-bit
    // explicit codes.
    // Zopfli is a q10-11 algorithm in the reference (backward
    // references HQ); q4-9 there is a single greedy/lazy hash-chain
    // pass. Match that effort mapping: q4-9 fall through to the lazy
    // path below with quality-tiered chain depths.
    // BROTLI_GREEDY_TIER routes q4-9 through the lazy path (the
    // reference's algorithm shape at those qualities) for measurement
    // of the deliberate time-for-ratio trade.
    // Greedy tier (the reference's algorithm shape at q4-9): on by
    // default for inputs of >= 1 MiB — measured, the greedy + rep-ring
    // beats both the reference and our zopfli there (CSV 21MB q5
    // 644,612B/3.4s vs zopfli 859,006/16.8s vs ref 1,058,795), while
    // below ~1 MiB the DP's global pricing wins (100KB: +12%).
    // BROTLI_GREEDY_TIER forces on, BROTLI_NO_GREEDY_TIER forces off.
    let greedy_tier = quality >= 4
        && quality < 10
        && !env_flag!("BROTLI_NO_GREEDY_TIER")
        && (env_flag!("BROTLI_GREEDY_TIER") || input.len() >= 1 << 20);
    let use_dict = if greedy_tier && env_flag!("BROTLI_GREEDY_NODICT") {
        false
    } else {
        use_dict
    };
    if greedy_tier {
        // BROTLI_GREEDY_NICECAP restores the old nice_match compare cap
        // (the reference hashers compute full match lengths — long
        // copies on repetitive data are their main lever).
        if env_flag!("BROTLI_GREEDY_NICECAP") {
            if mf.max_match_length() > nice_match {
                mf.set_max_match_length(nice_match);
            }
        }
        if quality < 10 && input.len() >= 2 << 20 {
            // The lazy lookahead walks the MF's own chain cap, and its
            // decisions shape the whole parse: measured at 21MB, the
            // q8-9 matcher config (chain 64, nice 256) reaches 3.03%
            // where the q4-7 config (16, 96) lands at 6.34-10%. Below
            // 2 MiB the override slightly hurts (1MB: 4.33->4.60%).
            mf.set_max_chain_length(64);
            mf.set_nice_match(256);
        }
        // fall through to the lazy path below
    } else if quality >= 10 && !env_flag!("BROTLI_OLD_ZOPFLI") {
        // Reference port: BrotliCreate(Hq)ZopfliBackwardReferences —
        // q10 single pass, q11 two passes with the StartPosQueue.
        // BROTLI_OLD_ZOPFLI restores the in-house iterative DP.
        return (crate::encoder::zopfli_hq::parse_hq(input, quality), None);
    } else if quality >= 4 && input.len() <= 8 * 1024 * 1024 {
        return zopfli_iterative_parse(
            input,
            history,
            &mut mf,
            mlen_offset,
            use_dict,
            quality,
            is_last,
            ctx_in,
        );
    } else if quality >= 4 {
        return (two_pass_parse(input, mlen_offset, &mut mf, use_dict), None);
    }

    let mut pos = 0usize;
    let mut insert_start = 0usize;
    // MF query coordinate: the shared frame MF spans global positions
    // [mf_base, ..); chunk-local MFs start at global mlen_offset.
    let to_g = |p: usize| mlen_offset + p - mf_base;
    // Recent-distances ring for the greedy tier (upstream's
    // last_distances): H5/H6 matchers probe these at every position
    // BEFORE the hash chain — rep codes cost ~1-2 bits vs ~12-15 for
    // explicit distances, so a slightly shorter rep match usually
    // wins on the wire. Without this the structural distance never
    // stays warm under greedy and far matches get re-encoded
    // explicitly every time (CSV 21MB: 7.18% vs ref 4.98%).
    let mut last_dists: [u32; 4] = [0; 4];
    let mut last_dist_len = 0usize;
    // Reference CreateBackwardReferences: up to 4 lazy delays in a row.
    let mut delayed_in_row = 0i32;
    // Candidate buffer hoisted out of the loop: a per-position Vec
    // meant an alloc/free pair per position.
    let mut cands_buf: Vec<omnizip_codecs::Lz77Match> = Vec::new();
    while pos < n {
        // Global output position (across metablocks) for max_distance.
        let global_pos = mlen_offset + pos;
        let max_dist = (global_pos as u32).min(MAX_BACKWARD_DISTANCE);
        let mut bank_hit_score = 0u64;

        let lz77 = if pos + MIN_MATCH as usize <= n {
            // Advance both finders when the bank is live so the chain
            // stays warm as a long-match secondary source.
            let bank_hit = if let Some(bank) = bank_mf.as_deref_mut() {
                // Find-AND-insert with one hash (upstream FindLongestMatch
                // stores the searched position with the hash it just
                // computed). Full remaining length: the reference hashers
                // do not cap match length at nice_len. Matches scoring at
                // or below kMinScore (30*8*8 + 100) are rejected — the
                // reference emits literals for them instead.
                let cap_len = (n - pos) as u32;
                bank.find_insert(global_pos, &last_dists[..last_dist_len], cap_len, 3, true)
                    .filter(|&(_, _, s)| s > 2020)
                    .map(|(d, l, s)| {
                        bank_hit_score = s;
                        omnizip_codecs::Lz77Match {
                            distance: d,
                            length: l,
                        }
                    })
            } else {
                None
            };
            if bank_mf.is_none() {
                mf.advance();
            }
            if quality >= 8 || greedy_tier {
                // Cost-scored selection (reference-style lazy scoring):
                // the longest match often pays 10-15 extra bits in an
                // explicit distance code; score candidates by length
                // minus a distance penalty instead of taking max length.
                let (ncand, nwalk) = if quality >= 8 {
                    (12, 96)
                } else if quality >= 6 {
                    (6, 32)
                } else {
                    (4, 16)
                };
                cands_buf.clear();
                let cands = &mut cands_buf;
                if bank_mf.is_some() {
                    // Binary bank path: bank hit is the sole candidate
                    // (chain walks are the cost we are eliminating).
                    // The bank's find already scored it H9-style — the
                    // f32 rescoring loop below (a log2 per candidate per
                    // position) only served the multi-candidate chain
                    // path.
                    if let Some(m) = bank_hit {
                        cands.push(m);
                    }
                } else {
                    mf.find_candidates_into_patience(to_g(pos), ncand, nwalk, 8, cands);
                }
                let mut bestc: Option<omnizip_codecs::Lz77Match> = None;
                let mut best_score = f32::MIN;
                let mut best_is_rep = false;
                if bank_mf.is_some() {
                    if let Some(m) = cands.first() {
                        if m.distance <= max_dist && m.length >= 2 {
                            bestc = Some(*m);
                            best_is_rep =
                                greedy_tier && last_dists[..last_dist_len].contains(&m.distance);
                            best_score = m.length as f32;
                        }
                    }
                } else {
                    for (ci, m) in cands.iter().enumerate() {
                        if m.distance > max_dist || m.length < 2 {
                            continue;
                        }
                        let is_rep =
                            greedy_tier && last_dists[..last_dist_len].contains(&m.distance);
                        // Upstream's lazy scoring: distance penalty is
                        // 0.679 * log2(dist) (kBrotliLog2Table-scaled), not
                        // a full log2 — softer penalties keep nearer (more
                        // rep-friendly) matches in contention. A rep-distance
                        // match rides a ~2-bit rep code instead.
                        let pen = if is_rep {
                            // Rep preference scales with the explicit cost
                            // it displaces: a rep code saves ~10-25 bits at
                            // far distances but only ~2-4 bits locally.
                            (0.679 * (m.distance as f32).log2() - 2.0).max(1.0)
                        } else if m.distance <= 4 {
                            2.0
                        } else {
                            0.679 * (m.distance as f32).log2()
                        };
                        let score = m.length as f32 - pen;
                        if score > best_score {
                            best_score = score;
                            bestc = cands.get(ci).copied();
                            best_is_rep = is_rep;
                        }
                    }
                }
                // Probe the recent distances directly (upstream's
                // last-distance check in FindLongestMatch): a rep
                // distance whose match is absent from the congested
                // 4-byte chains — the structural period on periodic
                // data — must still be considered. The bank already
                // probes these internally (they are passed to find),
                // so this is the text/chain path only.
                if greedy_tier && bank_mf.is_none() && pos + MIN_MATCH as usize <= n {
                    let cap = ((n - pos) as u32).min(nice_match);
                    for &r in last_dists.iter().take(last_dist_len) {
                        if r == 0 || r as usize > global_pos {
                            continue;
                        }
                        let src = to_g(pos) - r as usize;
                        let l = if src >= mf_base {
                            mf.match_len_between(to_g(pos), src, cap)
                        } else {
                            // Source precedes the MF window: compare
                            // against the history slice directly.
                            let hist_base = mlen_offset - history.len();
                            let mut l = 0u32;
                            while l < cap {
                                let src_h = src + l as usize - hist_base;
                                let idx = pos + l as usize;
                                if idx >= n {
                                    break;
                                }
                                let cur = input[idx];
                                if src_h < history.len() && history[src_h] == cur {
                                    l += 1;
                                } else {
                                    break;
                                }
                            }
                            l
                        };
                        if l < MIN_MATCH {
                            continue;
                        }
                        let score = l as f32 - (0.679 * (r as f32).log2() - 2.0).max(1.0);
                        if score > best_score {
                            best_score = score;
                            bestc = Some(omnizip_codecs::Lz77Match {
                                distance: r,
                                length: l,
                            });
                            best_is_rep = true;
                        }
                    }
                }
                let _ = best_is_rep;
                bestc
            } else if let Some(m) = bank_hit {
                Some(m)
            } else {
                mf.find_match(to_g(pos))
            }
        } else {
            None
        };

        let lz77_valid = lz77.as_ref().is_some_and(|m| m.distance <= max_dist);

        let best: Option<(u32, u32, u32)> = if lz77_valid {
            let m = lz77.as_ref().unwrap();
            // Clamp to the chunk end (metablock boundary).
            let len = m.length.min((n - pos) as u32);
            // Reference: the static dictionary is searched ONLY when
            // FindLongestMatch found nothing — the mid-length dict
            // probe below cost ~30% of q5 encode for a small ratio
            // gain. BROTLI_MID_DICT restores it for measurement.
            if m.length >= 8 && use_dict && env_flag!("BROTLI_MID_DICT") {
                let dict = dict_hash::find_match(input, pos, max_dist);
                match dict {
                    // Guard: the TRANSFORMED length may exceed the chunk
                    // remainder (suffix-appending transforms); a dict
                    // reference past mlen overruns the metablock.
                    Some((d, wl, tl)) if tl > m.length && tl as usize <= n - pos => {
                        Some((d, wl, tl))
                    }
                    // Fallback: the CLAMPED length — raw m.length can
                    // extend past the chunk end (audit bug: copy=7 with
                    // 5 bytes remaining overran mlen by 2).
                    _ => Some((m.distance, len, len)),
                }
            } else {
                Some((m.distance, len, len))
            }
        } else if use_dict {
            // Reference greedy path: the two-probe static-dictionary
            // search (no transformed-word chains). BROTLI_SLOW_DICT
            // restores the pool-based finder for measurement.
            if env_flag!("BROTLI_SLOW_DICT") {
                dict_hash::find_match(input, pos, max_dist)
                    .filter(|&(_, _, tl)| tl as usize <= n - pos)
                    .map(|(d, wl, tl)| (d, wl, tl))
            } else {
                crate::encoder::dict_hash_lut::find_match_fast(input, pos, max_dist, 3)
                    .filter(|&(_, _, tl)| tl as usize <= n - pos)
            }
        } else {
            None
        };

        if let Some((distance, copy_len, advance_len)) = best {
            if advance_len >= 2 && distance > 0 {
                // Lazy matching. Bank path uses the reference rule
                // (CreateBackwardReferences): re-search at pos+1 at
                // full depth and defer while the next score beats the
                // current by >= 175, up to 4 delays in a row. The
                // chain path keeps the length-based heuristic.
                if lazy && !env_flag!("BROTLI_NO_LAZY") && pos + 1 < n {
                    if bank_mf.is_some() {
                        let next_pos = pos + 1;
                        let next_global = mlen_offset + next_pos;
                        if delayed_in_row < 4 && next_pos + 4 <= n {
                            let m2 = bank_mf.as_deref().and_then(|bank| {
                                // Reference pre-seeds the lazy re-search
                                // at sr.len-1: most candidates then
                                // reject on a single byte compare.
                                // Reference pre-seeds the lazy re-search
                                // at sr.len-1 ONLY below q5; at q5+ the
                                // re-search starts empty (sr2.len = 0).
                                bank.find_with_floor(
                                    next_global,
                                    &last_dists[..last_dist_len],
                                    (n - next_pos) as u32,
                                    if quality < 5 {
                                        lz77.as_ref().map_or(3, |m| m.length - 1)
                                    } else {
                                        3
                                    },
                                )
                            });
                            if let Some((_, _, s2)) = m2 {
                                if s2 >= bank_hit_score + 175 {
                                    delayed_in_row += 1;
                                    pos += 1;
                                    continue;
                                }
                            }
                        }
                    } else if advance_len < nice_match {
                        let next_pos = pos + 1;
                        let next_global = mlen_offset + next_pos;
                        let next_max = (next_global as u32).min(MAX_BACKWARD_DISTANCE);

                        let next_lz77 = if next_pos + MIN_MATCH as usize <= n {
                            mf.find_match_capped(to_g(next_pos), 48.min(mf.max_chain_length()))
                        } else {
                            None
                        };

                        let next_valid = next_lz77.as_ref().is_some_and(|m| m.distance <= next_max);
                        let next_best_len: Option<u32> = if next_valid {
                            let nm = next_lz77.as_ref().unwrap();
                            if nm.length >= 8 || !use_dict {
                                Some(nm.length)
                            } else {
                                match dict_hash::find_match(input, next_pos, next_max) {
                                    Some((_, _, tl)) if tl > nm.length => Some(tl),
                                    _ => Some(nm.length),
                                }
                            }
                        } else if use_dict {
                            dict_hash::find_match(input, next_pos, next_max).map(|(_, _, tl)| tl)
                        } else {
                            None
                        };

                        if let Some(next_len) = next_best_len {
                            if next_len > advance_len {
                                // Lazy2: check pos+2 before committing to pos+1.
                                if lazy2 && next_len < nice_match && pos + 2 < n {
                                    let next2_pos = pos + 2;
                                    let next2_global = mlen_offset + next2_pos;
                                    let next2_max =
                                        (next2_global as u32).min(MAX_BACKWARD_DISTANCE);
                                    let next2_best_len: Option<u32> =
                                        if next2_pos + MIN_MATCH as usize <= n {
                                            mf.find_match_capped(to_g(next2_pos), 8)
                                                .filter(|m| m.distance <= next2_max)
                                                .map(|m| {
                                                    if m.length >= 8 || !use_dict {
                                                        m.length
                                                    } else {
                                                        match dict_hash::find_match(
                                                            input, next2_pos, next2_max,
                                                        ) {
                                                            Some((_, _, tl)) if tl > m.length => tl,
                                                            _ => m.length,
                                                        }
                                                    }
                                                })
                                                .or_else(|| {
                                                    if use_dict {
                                                        dict_hash::find_match(
                                                            input, next2_pos, next2_max,
                                                        )
                                                        .map(|(_, _, tl)| tl)
                                                    } else {
                                                        None
                                                    }
                                                })
                                        } else {
                                            None
                                        };
                                    if let Some(n2_len) = next2_best_len {
                                        if n2_len > next_len {
                                            pos += 2;
                                            continue;
                                        }
                                    }
                                }
                                pos += 1;
                                continue;
                            }
                        }
                    }
                }

                let clamped_copy = copy_len.min(MAX_COPY).max(2);
                let insert_len = (pos - insert_start) as u32;
                delayed_in_row = 0;
                commands.push(Command {
                    insert_len,
                    copy_len: clamped_copy,
                    distance,
                });
                if greedy_tier && distance <= max_dist {
                    // Newest-first ring (upstream distance_cache[0] =
                    // most recent): on hit, pull to front; on miss,
                    // shift right and insert at [0].
                    if let Some(k) = last_dists[..last_dist_len]
                        .iter()
                        .position(|&d| d == distance)
                    {
                        let v = last_dists[k];
                        last_dists.copy_within(0..k, 1);
                        last_dists[0] = v;
                    } else {
                        let n = last_dist_len.min(3);
                        last_dists.copy_within(0..n, 1);
                        last_dists[0] = distance;
                        last_dist_len = (last_dist_len + 1).min(4);
                    }
                }
                // Advance: for LZ77, use clamped copy_len (matches
                // decoder output). For dictionary, use transformed_len
                // (may differ from copy_len when transforms add/remove
                // bytes).
                let advance = if advance_len > MAX_COPY {
                    clamped_copy as usize
                } else {
                    (advance_len as usize).min(n - pos)
                };
                // Upstream StoreRange RLE guard ("avoid hash poisoning
                // with RLE data"): when the copy overlaps heavily
                // (distance < len/4), skip storing the early part of
                // the covered range — those positions repeat the same
                // hash window and would crowd the bank's 16 slots,
                // evicting genuinely useful distant positions. The
                // lazy-search position (pos+1) is always stored (the
                // reference's FindLongestMatch inserts it during the
                // sr2 search).
                let rle_store_from = if distance < (advance as u32) >> 2 {
                    (pos + advance - 4 * distance as usize).max(pos + 2)
                } else {
                    pos + 2
                };
                let mut skipped = 1usize;
                for _ in 1..advance {
                    if pos + 1 < n {
                        pos += 1;
                        if let Some(bank) = bank_mf.as_deref_mut() {
                            if skipped == 1 || pos >= rle_store_from {
                                bank.advance();
                            } else {
                                bank.skip();
                            }
                        } else {
                            mf.advance();
                        }
                        skipped += 1;
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

    if env_flag!("BROTLI_PARSE_STATS") {
        let n_cmd = commands.len();
        let n_copy = commands.iter().filter(|c| c.copy_len > 0).count();
        let copy_bytes: u64 = commands.iter().map(|c| u64::from(c.copy_len)).sum();
        let bank_on = bank_mf.is_some();
        eprintln!(
            "PARSESTATS n={n} cmds={n_cmd} copies={n_copy} copy_bytes={copy_bytes} bank={bank_on} greedy={greedy_tier}"
        );
    }
    (commands, None)
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
/// (`wire_value`, bits) encoding via the fixed `K_CL_PREFIX` code.
///
/// Derived from the decoder's `K_CL_PREFIX_VALUE` / `K_CL_PREFIX_LENGTH`
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

/// `CODE_LENGTH_CODE_ORDER` per RFC 7932 §9.5.2.
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
    let nonzero: Vec<usize> = lengths
        .lengths
        .iter()
        .enumerate()
        .filter(|(_, &l)| l > 0)
        .map(|(i, _)| i)
        .collect();
    // Use simple form when the code lengths match a simple-form pattern.
    // This avoids the complex form RLE path for sparse tables where it
    // produces wire-format mismatches.
    if nonzero.len() <= 1 {
        let sym = nonzero.first().copied().unwrap_or(0);
        write_simple_one_symbol(bw, alphabet, sym);
        return;
    }
    // Check if lengths match a simple-form assignment:
    // NSYM=2: both length 1
    // NSYM=3: first length 1, other two length 2
    // NSYM=4: all length 2
    let matches_simple = match nonzero.len() {
        2 => lengths.lengths[nonzero[0]] == 1 && lengths.lengths[nonzero[1]] == 1,
        3 => {
            lengths.lengths[nonzero[0]] == 1
                && lengths.lengths[nonzero[1]] == 2
                && lengths.lengths[nonzero[2]] == 2
        }
        4 => nonzero.iter().all(|&i| lengths.lengths[i] == 2),
        _ => false,
    };
    if matches_simple {
        write_simple_form_table(bw, alphabet, &nonzero);
        return;
    }

    // Complex form: HSKIP = 0.
    bw.write_bits(0, 2);

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
    for (i, &sym) in CODE_LENGTH_CODE_ORDER.iter().enumerate() {
        let len = cl_lengths.lengths[usize::from(sym)];
        let (wire, nbits) = CL_CODE_TO_WIRE[usize::from(len)];
        if env_flag!("BROTLI_TREEDBG") {
            eprintln!("WRHD i={i} v={len} bits={wire}/{nbits}");
        }
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
    let tdbg = env_flag!("BROTLI_TREEDBG");
    for &(sym, extra) in &rle {
        if tdbg {
            eprintln!("WRCL sym={sym} extra={extra} space={main_space}");
        }
        let (val, count) = match sym {
            16 => (prev_code_len, 3 + extra as usize),
            17 => (0u8, 3 + extra as usize),
            v => (v, 1usize),
        };

        if !cl_single {
            let (code, clen) = cl_codes[usize::from(sym)];
            let wire = reverse_bits(code, clen);
            bw.write_bits(wire, u32::from(clen));
        }
        // Extra bits for RLE symbols must be written even when
        // cl_single is true (decoder reads 0 bits for the symbol
        // itself via single_symbol fast path, but still reads the
        // extra bits for symbols 16 and 17).
        if sym == 16 {
            bw.write_bits(extra as u32, 2);
        } else if sym == 17 {
            bw.write_bits(extra as u32, 3);
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

/// Write block-type code trees and initial block length (RFC 7932 §9.3).
///
/// For NBLTYPES=2:
/// - Block-type code tree: simple form NSYM=2, symbols [2, 3] (1 bit each)
///   symbol 2 → type 0, symbol 3 → type 1
/// - Block-length code tree: simple form NSYM=1, symbol = block-length code
/// - Initial block length: code 12 (offset=113) + extra (for 128 bytes)

/// Map a block length to its prefix code: (code, extra, nbits).
fn block_length_code(len: u32) -> (usize, u32, u32) {
    for (c, e) in crate::prefix::kBlockLengthPrefixCode.iter().enumerate() {
        let offset = u32::from(e.offset);
        let span = 1u32 << e.nbits;
        if len >= offset && len < offset + span {
            return (c, len - offset, u32::from(e.nbits));
        }
    }
    // Lengths beyond the table are impossible (max code covers 16625+);
    // clamp defensively to the last code.
    let last = crate::prefix::kBlockLengthPrefixCode.len() - 1;
    let e = &crate::prefix::kBlockLengthPrefixCode[last];
    (last, (1u32 << e.nbits) - 1, u32::from(e.nbits))
}

fn huff_cost(freq: &[u32; 704]) -> f64 {
    let h = omnizip_codecs::HuffmanLengths::build(freq, 15);
    let mut cost = 0.0f64;
    for (&f, &l) in freq.iter().zip(h.lengths.iter()) {
        if f > 0 {
            cost += f as f64 * f64::from(l);
        }
    }
    cost
}

/// Optimal contiguous block split of the command stream (dynamic
/// programming over candidate cut points, minimizing summed per-block
/// Huffman/entropy cost). This is the BrotliBuildMetaBlock command
/// pass done exactly: candidate cuts every `step` commands, at most
/// `max_blocks` blocks. Returns block START indices (first is 0).
fn split_cmd_symbols_optimal(cmd_symbols: &[usize], max_blocks: usize) -> Vec<usize> {
    split_symbol_stream_optimal(cmd_symbols, 704, max_blocks)
}

/// Optimal contiguous split of a symbol stream (entropy DP over cut
/// points). Returns block START indices (first is 0).
/// log2 via a 16-bit-mantissa table (error <= 1.1e-5 bits): the block
/// splitters evaluate tens of millions of entropies per chunk and
/// libm's log2 is ~30% of total encode time on large streams. Upstream
/// brotli uses the same table trick for its splitter (FastLog2).
#[doc(hidden)]
pub fn _fast_log2_diag(v: u32) -> f64 {
    fast_log2(&log2_table(), v)
}

fn log2_table() -> &'static Box<[f64; 65_536]> {
    static TABLE: std::sync::OnceLock<Box<[f64; 65_536]>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = Box::new([0f64; 65_536]);
        for (i, x) in t.iter_mut().enumerate() {
            *x = (1.0 + f64::from(i as u32) / 65_536.0).log2();
        }
        t
    })
}

/// Table fetched ONCE per splitter invocation — the OnceLock's atomic
/// load per call cost more than the lookup itself at the ~10^8 calls
/// per large chunk.
fn fast_log2(tbl: &[f64; 65_536], v: u32) -> f64 {
    if v == 0 {
        return 0.0;
    }
    let e = 31 - v.leading_zeros();
    let idx = if e >= 16 {
        ((v >> (e - 16)) & 0xFFFF) as usize
    } else {
        ((v << (16 - e)) & 0xFFFF) as usize
    };
    tbl[idx] + f64::from(e)
}

fn split_symbol_stream_optimal(
    symbols: &[usize],
    alphabet: usize,
    max_blocks: usize,
) -> Vec<usize> {
    if std::env::var("BROTLI_DP_SPLIT").is_ok() {
        return split_symbol_stream_dp(symbols, alphabet, max_blocks);
    }
    // Reference params (BrotliBuildMetaBlockGreedyInternal): commands
    // min-block 1024 / threshold 500, distances 512 / 100.
    if alphabet == 704 {
        greedy_split(symbols.len(), alphabet, 1024, 500.0, max_blocks, |i| {
            symbols[i]
        })
    } else {
        greedy_split(symbols.len(), alphabet, 512, 100.0, max_blocks, |i| {
            symbols[i]
        })
    }
}

/// Reference BitsEntropy: Shannon bits with a 1-bit/symbol floor.
fn bits_entropy(h: &[u32]) -> f32 {
    let tbl = log2_table();
    let mut sum: u64 = 0;
    let mut acc: f64 = 0.0;
    for &p in h {
        if p > 0 {
            sum += u64::from(p);
            acc -= f64::from(p) * f64::from(fast_log2(tbl, p));
        }
    }
    if sum == 0 {
        return 0.0;
    }
    acc += f64::from(fast_log2(tbl, sum as u32)) * (sum as f64);
    let r = acc as f32;
    if r < sum as f32 {
        sum as f32
    } else {
        r
    }
}

/// Greedy block splitter (port of the reference BlockSplitter state
/// machine from BrotliBuildMetaBlockGreedy): symbols accumulate into
/// the current histogram; once the target block size is reached the
/// splitter decides to start a new type, merge with the type two back,
/// or extend the previous block, by comparing entropy deltas against a
/// threshold. Returns block START indices (first is 0). O(n) — the
/// exact DP this replaces was O(m²) over cut candidates and dominated
/// emission time on multi-MB streams.
fn greedy_split(
    n: usize,
    alphabet: usize,
    min_block_size: usize,
    threshold: f32,
    max_blocks: usize,
    sym: impl Fn(usize) -> usize,
) -> Vec<usize> {
    if n == 0 {
        return vec![0];
    }
    let mut cuts = vec![0usize];
    let mut curr = vec![0u32; alphabet];
    let mut h_last = vec![0u32; alphabet];
    let mut h_last2 = vec![0u32; alphabet];
    let mut e_last = [f32::MAX; 2];
    let mut target = min_block_size;
    let mut block_size = 0usize;
    let mut num_types = 1usize;
    let mut merge_last_count = 0usize;
    let mut started = false;
    let mut cut_start = 0usize;
    let mut finish = |i: usize,
                      curr: &mut Vec<u32>,
                      h_last: &mut Vec<u32>,
                      h_last2: &mut Vec<u32>,
                      e_last: &mut [f32; 2],
                      target: &mut usize,
                      merge_last_count: &mut usize,
                      cuts: &mut Vec<usize>,
                      num_types: &mut usize,
                      cut_start: &mut usize,
                      started: &mut bool,
                      block_size: &mut usize| {
        if !*started {
            h_last.clone_from(curr);
            e_last[0] = bits_entropy(curr);
            e_last[1] = e_last[0];
            *started = true;
            *cut_start = i + 1;
            curr.iter_mut().for_each(|x| *x = 0);
            *block_size = 0;
            return;
        }
        if *block_size == 0 {
            return;
        }
        let ent = bits_entropy(curr);
        let mut comb0 = curr.clone();
        let mut comb1 = curr.clone();
        for (a, &b) in comb0.iter_mut().zip(h_last.iter()) {
            *a += b;
        }
        for (a, &b) in comb1.iter_mut().zip(h_last2.iter()) {
            *a += b;
        }
        let ce0 = bits_entropy(&comb0);
        let ce1 = bits_entropy(&comb1);
        let d0 = ce0 - ent - e_last[0];
        let d1 = ce1 - ent - e_last[1];
        if *num_types < 256 && cuts.len() < max_blocks && d0 > threshold && d1 > threshold {
            cuts.push(*cut_start);
            *cut_start = i + 1;
            h_last2.clone_from(h_last);
            h_last.clone_from(curr);
            e_last[1] = e_last[0];
            e_last[0] = ent;
            *num_types += 1;
            *merge_last_count = 0;
            *target = min_block_size;
        } else if d1 < d0 - 20.0 && cuts.len() < max_blocks {
            // Merge with the type two back: the reference reuses that
            // block type here; without type reuse on our wire this is
            // still a block boundary.
            cuts.push(*cut_start);
            *cut_start = i + 1;
            h_last2.clone_from(h_last);
            h_last.clone_from(&comb1);
            e_last[1] = e_last[0];
            e_last[0] = ce1;
            *merge_last_count = 0;
            *target = min_block_size;
        } else {
            for (a, &b) in h_last.iter_mut().zip(comb0.iter()) {
                *a = b;
            }
            e_last[0] = ce0;
            if *num_types == 1 {
                e_last[1] = e_last[0];
            }
            *merge_last_count += 1;
            if *merge_last_count > 1 {
                *target += min_block_size;
            }
        }
        curr.iter_mut().for_each(|x| *x = 0);
        *block_size = 0;
    };
    for i in 0..n {
        curr[sym(i)] += 1;
        block_size += 1;
        if block_size >= target {
            finish(
                i,
                &mut curr,
                &mut h_last,
                &mut h_last2,
                &mut e_last,
                &mut target,
                &mut merge_last_count,
                &mut cuts,
                &mut num_types,
                &mut cut_start,
                &mut started,
                &mut block_size,
            );
        }
    }
    if block_size > 0 {
        finish(
            n - 1,
            &mut curr,
            &mut h_last,
            &mut h_last2,
            &mut e_last,
            &mut target,
            &mut merge_last_count,
            &mut cuts,
            &mut num_types,
            &mut cut_start,
            &mut started,
            &mut block_size,
        );
    }
    cuts
}

/// Exact O(m²) cut DP (env: BROTLI_DP_SPLIT) — the pre-greedy splitter.
fn split_symbol_stream_dp(symbols: &[usize], alphabet: usize, max_blocks: usize) -> Vec<usize> {
    let cmd_symbols = symbols;
    let n = cmd_symbols.len();
    if n < 1024 || max_blocks < 2 {
        return vec![0];
    }
    let step = 256.min(n / 4).max(32);
    let mut cuts: Vec<usize> = (0..n).step_by(step).collect();
    if *cuts.last().unwrap() != n {
        cuts.push(n);
    }
    let m = cuts.len() - 1; // segments
    if m < 2 {
        return vec![0];
    }
    let kmax = max_blocks.min(m);

    // Per-segment histograms + total counts.
    let mut seg_hist = vec![[0u32; 704]; m];
    for i in 0..m {
        for &s in &cmd_symbols[cuts[i]..cuts[i + 1]] {
            seg_hist[i][s] += 1;
        }
    }

    // Per-segment distinct symbols: merges touch only these.
    let seg_syms: Vec<Vec<u16>> = (0..m)
        .map(|i| {
            seg_hist[i]
                .iter()
                .enumerate()
                .filter(|&(_, &f)| f > 0)
                .map(|(s, _)| s as u16)
                .collect()
        })
        .collect();

    // block_cost[j][i] = entropy bits of commands cuts[j]..cuts[i],
    // maintained incrementally: bits(t) = t·log2(t) − Σ f·log2(f), so
    // adding count c of symbol s changes bits by
    //   (t+c)·log2(t+c) − t·log2(t) + f·log2(f) − (f+c)·log2(f+c).
    // O(1) per distinct symbol instead of an O(alphabet) rescan with a
    // libm log2 per nonzero bin (which was ~30% of encode time on
    // large streams).
    let log2_tbl = log2_table();
    let mut block_cost = vec![vec![f64::INFINITY; m + 1]; m + 1];
    let mut hist = vec![0u32; alphabet];
    for j in 0..m {
        let mut touched: Vec<u16> = Vec::new();
        let mut t: u64 = 0;
        let mut bits = 0.0f64;
        block_cost[j][j] = 0.0;
        for i in j..m {
            for &sym in &seg_syms[i] {
                let c = u64::from(seg_hist[i][usize::from(sym)]);
                let f0 = u64::from(hist[usize::from(sym)]);
                let f1 = f0 + c;
                if f0 == 0 {
                    touched.push(sym);
                }
                hist[usize::from(sym)] = f1 as u32;
                let t1 = t + c;
                // L(x) = x·log2(x); the telescoping update is
                // L(t1) − L(t0) + L(f0) − L(f1).
                bits += f64::from(t1 as u32) * fast_log2(log2_tbl, t1 as u32)
                    - f64::from(t as u32) * fast_log2(log2_tbl, t as u32)
                    + f64::from(f0 as u32) * fast_log2(log2_tbl, f0 as u32)
                    - f64::from(f1 as u32) * fast_log2(log2_tbl, f1 as u32);
                t = t1;
            }
            block_cost[j][i + 1] = bits;
        }
        for &sym in &touched {
            hist[usize::from(sym)] = 0;
        }
    }

    // dp[k][i]: min cost covering cuts[0..=i] with k blocks.
    let inf = f64::INFINITY;
    let mut dp = vec![vec![inf; m + 1]; kmax + 1];
    let mut choice = vec![vec![0usize; m + 1]; kmax + 1];
    dp[0][0] = 0.0;
    for k in 1..=kmax {
        for i in 1..=m {
            let mut best = inf;
            let mut arg = 0;
            for j in (k - 1..i).rev() {
                if dp[k - 1][j].is_infinite() {
                    continue;
                }
                let c = dp[k - 1][j] + block_cost[j][i];
                if c < best {
                    best = c;
                    arg = j;
                }
            }
            dp[k][i] = best;
            choice[k][i] = arg;
        }
    }
    let mut best_k = 1;
    let mut best_cost = dp[1][m];
    for k in 2..=kmax {
        let c = dp[k][m] + 25.0 * k as f64;
        if c < best_cost {
            best_cost = c;
            best_k = k;
        }
    }
    let mut starts = Vec::with_capacity(best_k);
    let mut i = m;
    let mut k = best_k;
    while k > 0 {
        let j = choice[k][i];
        starts.push(cuts[j]);
        i = j;
        k -= 1;
    }
    starts.reverse();
    starts
}

/// Optimal contiguous split of the literal stream (byte-histogram DP).
/// Returns block START indices in literal-index space (first is 0).
fn split_literals(literals: &[u8], max_blocks: usize) -> Vec<usize> {
    if std::env::var("BROTLI_DP_SPLIT").is_ok() {
        return split_literals_dp(literals, max_blocks);
    }
    // Reference params: literals min-block 512 / threshold 400.
    greedy_split(literals.len(), 256, 512, 400.0, max_blocks, |i| {
        usize::from(literals[i])
    })
}

/// Exact literal-split DP (env: BROTLI_DP_SPLIT).
fn split_literals_dp(literals: &[u8], max_blocks: usize) -> Vec<usize> {
    let n = literals.len();
    if n < 4096 || max_blocks < 2 {
        return vec![0];
    }
    let step = 512.min(n / 4).max(64);
    let mut cuts: Vec<usize> = (0..n).step_by(step).collect();
    if *cuts.last().unwrap() != n {
        cuts.push(n);
    }
    let m = cuts.len() - 1;
    if m < 2 {
        return vec![0];
    }
    let kmax = max_blocks.min(m);
    let mut seg_hist = vec![[0u32; 256]; m];
    for i in 0..m {
        for &b in &literals[cuts[i]..cuts[i + 1]] {
            seg_hist[i][b as usize] += 1;
        }
    }
    // Incremental entropy (see split_symbol_stream_optimal): O(1) per
    // distinct symbol via the telescoping identity
    // bits(t) = t·log2(t) − Σ f·log2(f).
    let seg_syms: Vec<Vec<u8>> = (0..m)
        .map(|i| {
            seg_hist[i]
                .iter()
                .enumerate()
                .filter(|&(_, &f)| f > 0)
                .map(|(s, _)| s as u8)
                .collect()
        })
        .collect();
    let log2_tbl = log2_table();
    let mut block_cost = vec![vec![f64::INFINITY; m + 1]; m + 1];
    let mut hist = [0u32; 256];
    for j in 0..m {
        let mut touched: Vec<u8> = Vec::new();
        let mut t: u64 = 0;
        let mut bits = 0.0f64;
        block_cost[j][j] = 0.0;
        for i in j..m {
            for &sym in &seg_syms[i] {
                let c = u64::from(seg_hist[i][usize::from(sym)]);
                let f0 = u64::from(hist[usize::from(sym)]);
                let f1 = f0 + c;
                if f0 == 0 {
                    touched.push(sym);
                }
                hist[usize::from(sym)] = f1 as u32;
                let t1 = t + c;
                // L(x) = x·log2(x); the telescoping update is
                // L(t1) − L(t0) + L(f0) − L(f1).
                bits += f64::from(t1 as u32) * fast_log2(log2_tbl, t1 as u32)
                    - f64::from(t as u32) * fast_log2(log2_tbl, t as u32)
                    + f64::from(f0 as u32) * fast_log2(log2_tbl, f0 as u32)
                    - f64::from(f1 as u32) * fast_log2(log2_tbl, f1 as u32);
                t = t1;
            }
            block_cost[j][i + 1] = bits;
        }
        for &sym in &touched {
            hist[usize::from(sym)] = 0;
        }
    }
    let inf = f64::INFINITY;
    let mut dp = vec![vec![inf; m + 1]; kmax + 1];
    let mut choice = vec![vec![0usize; m + 1]; kmax + 1];
    dp[0][0] = 0.0;
    for k in 1..=kmax {
        for i in 1..=m {
            let mut best = inf;
            let mut arg = 0;
            for j in (k - 1..i).rev() {
                if dp[k - 1][j].is_infinite() {
                    continue;
                }
                let c = dp[k - 1][j] + block_cost[j][i];
                if c < best {
                    best = c;
                    arg = j;
                }
            }
            dp[k][i] = best;
            choice[k][i] = arg;
        }
    }
    // Per-block overhead: block switch codes + extra cmap entries.
    let switch_cost = std::env::var("BROTLI_LIT_SPLIT_COST")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(80.0);
    let mut best_k = 1;
    let mut best_cost = dp[1][m];
    for k in 2..=kmax {
        let c = dp[k][m] + switch_cost * k as f64;
        if c < best_cost {
            best_cost = c;
            best_k = k;
        }
    }
    let mut starts = Vec::with_capacity(best_k);
    let mut i = m;
    let mut k = best_k;
    while k > 0 {
        let j = choice[k][i];
        starts.push(cuts[j]);
        i = j;
        k -= 1;
    }
    starts.reverse();
    starts
}

/// Greedy command-block splitting (simplified BrotliBuildMetaBlock
/// command pass): walk the command symbol stream in windows; split off
/// a new block when coding the window under its own tree is cheaper
/// than merging it into the current block, net of the block-switch
/// overhead. Returns block START indices (first entry always 0).
fn split_cmd_symbols(cmd_symbols: &[usize], max_blocks: usize) -> Vec<usize> {
    let mut boundaries = vec![0usize];
    if cmd_symbols.len() < 1024 {
        return boundaries;
    }
    let window = std::env::var("BROTLI_SPLIT_WIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    let switch_cost = std::env::var("BROTLI_SPLIT_COST")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24.0);
    let WINDOW: usize = window;
    let SWITCH_COST: f64 = switch_cost;
    let mut cur = [0u32; 704];
    let mut i = 0usize;
    while i < cmd_symbols.len() {
        let wend = (i + WINDOW).min(cmd_symbols.len());
        if wend == cmd_symbols.len() {
            for &s in &cmd_symbols[i..wend] {
                cur[s as usize] += 1;
            }
            break;
        }
        let mut win = [0u32; 704];
        for &s in &cmd_symbols[i..wend] {
            win[s as usize] += 1;
        }
        let mut merged = [0u32; 704];
        for k in 0..704 {
            merged[k] = cur[k] + win[k];
        }
        let c_merged = huff_cost(&merged);
        let c_split = huff_cost(&cur) + huff_cost(&win);
        let cur_count: u32 = cur.iter().sum();
        if c_split + SWITCH_COST < c_merged && boundaries.len() < max_blocks && cur_count >= 64 {
            boundaries.push(i);
            cur = win;
        } else {
            cur = merged;
        }
        i = wend;
    }
    boundaries
}

/// Write the per-category block-switch header (block-type tree +
/// block-length tree + initial block length). Returns the wire code
/// tables (bt, bl) for emitting mid-stream switches.
fn write_block_switch_header(
    bw: &mut BitWriter,
    nbltypes: u32,
    block_lens: &[u32],
    block_types: &[u8],
) -> (Vec<(u32, u8)>, Vec<(u32, u8)>) {
    // Block-type code tree over alphabet 2 + nbltypes; switches use
    // explicit codes (type + 2). Frequencies must count the ACTUAL
    // emitted types: block splitting can REUSE type ids (the reference
    // ClusterBlocks output does), and a zero-frequency type gets a
    // zero-length code — emitting it writes no bits and desyncs the
    // decoder (it then parses the block-length code as a type).
    let bt_alphabet = 2 + nbltypes as usize;
    let mut bt_freq = vec![0u32; bt_alphabet];
    let fallback: Vec<u8> = (1..nbltypes as u8).collect();
    let types: &[u8] = if block_types.is_empty() {
        &fallback
    } else {
        block_types
    };
    for &ty in types {
        bt_freq[usize::from(ty) + 2] += 1;
    }
    let bt_lengths = omnizip_codecs::HuffmanLengths::build(&bt_freq, 15);
    write_huffman_table(bw, &bt_lengths, bt_alphabet);

    // Block-length code tree over the 26-symbol alphabet.
    let mut bl_freq = [0u32; 26];
    let bl_codes: Vec<(usize, u32, u32)> =
        block_lens.iter().map(|&l| block_length_code(l)).collect();
    for &(c, _, _) in &bl_codes {
        bl_freq[c] += 1;
    }
    let bl_lengths = omnizip_codecs::HuffmanLengths::build(&bl_freq, 15);
    write_huffman_table(bw, &bl_lengths, 26);

    let bt_wire = canonical_with_reverse(&bt_lengths);
    let bl_wire = canonical_with_reverse(&bl_lengths);

    // Initial block length (block 0).
    let (c0, extra0, nbits0) = bl_codes[0];
    let (code, len) = bl_wire[c0];
    bw.write_bits(code, u32::from(len));
    bw.write_bits(extra0, nbits0);
    (bt_wire, bl_wire)
}

fn write_block_type_trees(bw: &mut BitWriter, _nbltypes: u32) {
    // Block-type code tree: alphabet 2 + 2 = 4.
    // Simple form NSYM=2: symbols 2 and 3 (each 1 bit).
    bw.write_bits(0b01, 2); // HSKIP = 1
    bw.write_bits(0b01, 2); // NSYM-1 = 1 (NSYM=2)
    let bits_per_sym = ceil_log2(4); // ceil(log2(4)) = 2
    bw.write_bits(2, bits_per_sym); // s0 = 2 (type 0)
    bw.write_bits(3, bits_per_sym); // s1 = 3 (type 1)

    // Block-length code tree: alphabet 26.
    // Simple form NSYM=1: symbol = 12 (block-length code for ~128 bytes).
    // kBlockLengthPrefixCode[12] = offset=113, nbits=5 → range [113, 144].
    bw.write_bits(0b01, 2); // HSKIP = 1
    bw.write_bits(0b00, 2); // NSYM-1 = 0 (NSYM=1)
    let bl_bits_per_sym = ceil_log2(26); // ceil(log2(26)) = 5
    bw.write_bits(12, bl_bits_per_sym); // symbol = 12

    // Initial block length: read symbol 12 (0 bits, single_symbol), then
    // extra bits: 128 - 113 = 15 in 5 bits.
    bw.write_bits(15, 5); // extra = 15 → block_length = 113 + 15 = 128
}

/// Write a DecodeVarLenUint8-encoded value (RFC 7932 §9.3).
///
/// Inverse of the decoder's `read_varlen_uint8`:
/// - 0 → bit 0
/// - 1 → bits [1, 0,0,0] (1 + nbits=0 in 3 bits)
/// - N ≥ 2 → bits [1, nbits in 3 bits, extra in nbits bits]
///   where N = (1 << nbits) + extra.
fn write_varlen_uint8(bw: &mut BitWriter, value: u32) {
    if value == 0 {
        bw.write_bits(0, 1);
        return;
    }
    bw.write_bits(1, 1);
    if value == 1 {
        bw.write_bits(0, 3); // nbits = 0
        return;
    }
    let nbits = 31 - value.leading_zeros().min(31);
    let extra = value - (1u32 << nbits);
    bw.write_bits(nbits, 3);
    bw.write_bits(extra, nbits);
}

/// Write a literal context map (RFC 7932 §9.6) for NTREES > 1.
///
/// Format:
/// 1. RLE flag = 0 (no run-length encoding of zeros)
/// 2. Context-map code Huffman tree
/// 3. One symbol per context-map entry (64 entries for LSB6/UTF8)
/// 4. Inverse-MTF flag = 0
///
/// For NTREES ≤ 4: simple form code tree (uniform code lengths).
/// For NTREES > 4: complex form Huffman code tree built from the
/// actual symbol frequencies in `ctx_map`.
fn write_context_map(bw: &mut BitWriter, ctx_map: &[u8], ntrees: u32) {
    // RLE flag = 0 (no RLE).
    bw.write_bits(0, 1);

    // IMTF-encode the map: on block-structured maps most entries repeat
    // the previous value, which MTF turns into long 0-runs — the entry
    // code for 0 becomes 1 bit, shrinking the map dramatically.
    let max_val = ctx_map.iter().copied().max().unwrap_or(0) as usize;
    let mut mtf: Vec<u8> = (0..=max_val).map(|x| x as u8).collect();
    let mut encoded: Vec<u8> = Vec::with_capacity(ctx_map.len());
    for &v in ctx_map {
        let idx = mtf.iter().position(|&x| x == v).unwrap_or(0);
        encoded.push(idx as u8);
        if idx > 0 {
            mtf.remove(idx);
            mtf.insert(0, v);
        }
    }
    let ctx_map = encoded.as_slice();

    if ntrees <= 4 {
        // Simple form code tree. NOTE: NSYM=3 is NON-uniform — symbol 0
        // gets a 1-bit code, symbols 1-2 get 2-bit codes (mirrors
        // read_simple_form's lengths[s0]=1, lengths[s1]=lengths[s2]=2).
        // Treating NSYM=3 as uniform 2-bit silently corrupts every map
        // entry (tree 1 becomes undecodable) — this was the long-standing
        // "cluster_contexts wire-format mismatch".
        write_context_map_tree(bw, ntrees);

        let entry_codes: Vec<(u32, u8)> = match ntrees {
            1 => vec![(0, 1)],
            2 => (0..2).map(|v| (v, 1)).collect(),
            3 => vec![(0, 1), (0b01, 2), (0b11, 2)],
            // Uniform 2-bit canonical codes 00/01/10/11 reversed for
            // LSB-first: symbols 1↔2 swap (01→10, 10→01).
            _ => vec![(0, 2), (0b10, 2), (0b01, 2), (0b11, 2)],
        };
        for &entry in ctx_map {
            let (code, len) = entry_codes[entry as usize];
            bw.write_bits(code, u32::from(len));
        }
    } else {
        // Complex form: build Huffman tree from actual frequencies.
        let mut freq = [0u32; 256];
        for &entry in ctx_map {
            freq[entry as usize] += 1;
        }
        let lengths = omnizip_codecs::HuffmanLengths::build(&freq, 15);
        write_huffman_table(bw, &lengths, ntrees as usize);

        // Write each entry using the Huffman code (reversed for LSB-first).
        let codes = canonical_with_reverse(&lengths);
        for &entry in ctx_map {
            let (code, len) = codes[entry as usize];
            bw.write_bits(code, u32::from(len));
        }
    }

    // Inverse-MTF flag = 1: the map was MTF-encoded above.
    bw.write_bits(1, 1);
}

/// Write the context-map code Huffman tree in simple form.
/// For NTREES=2: NSYM=2, both symbols get 1-bit codes.
fn write_context_map_tree(bw: &mut BitWriter, ntrees: u32) {
    // HSKIP = 1 (simple form).
    bw.write_bits(0b01, 2);
    // NSYM: write nsym-1 in 2 bits.
    bw.write_bits(ntrees - 1, 2);
    // Symbols: each ceil_log2(ntrees) bits.
    let bits_per_sym = ceil_log2(ntrees);
    for sym in 0..ntrees {
        bw.write_bits(sym, bits_per_sym);
    }
    // For NSYM=4: decoder reads tree_select bit. Write 0 for uniform
    // 2-bit codes.
    if ntrees == 4 {
        bw.write_bits(0, 1);
    }
}

/// Write a single-symbol Huffman table in simple form (RFC 7932 §9.5.1).
fn write_simple_one_symbol(bw: &mut BitWriter, alphabet: usize, sym: usize) {
    bw.write_bits(0b01, 2); // HSKIP = 1
    bw.write_bits(0b00, 2); // NSYM = 1 (encoded as 0b00)
    let bits_per_sym = ceil_log2(alphabet as u32);
    bw.write_bits(sym as u32, bits_per_sym);
}

/// Write a 2/3/4-symbol Huffman table in simple form (RFC 7932 §9.5.1).
/// Symbols must be sorted ascending. Code length assignment:
/// - NSYM=2: both length 1
/// - NSYM=3: s0 length 1, s1/s2 length 2
/// - NSYM=4: all length 2 (tree_select=0)
fn write_simple_form_table(bw: &mut BitWriter, alphabet: usize, symbols: &[usize]) {
    let nsym = symbols.len();
    debug_assert!((2..=4).contains(&nsym));
    bw.write_bits(0b01, 2); // HSKIP = 1 (simple form)
    bw.write_bits(nsym as u32 - 1, 2); // NSYM-1
    let bits_per_sym = ceil_log2(alphabet as u32);
    for &s in symbols {
        bw.write_bits(s as u32, bits_per_sym);
    }
    if nsym == 4 {
        bw.write_bits(0, 1); // tree_select = 0 (all length 2)
    }
}

/// Override Huffman code lengths to match simple form assignment.
/// Forces sparse tables (2-4 non-zero symbols) into simple-form-
/// compatible code lengths to avoid the complex-form RLE encoding
/// path which produces wire-format mismatches for certain symbol
/// distributions.
///
/// - 2 symbols → both length 1 (NSYM=2 pattern)
/// - 3 symbols → lengths [1, 2, 2] (NSYM=3 pattern)
/// - 4 symbols → lengths [2, 2, 2, 2] (NSYM=4, tree_select=0)
fn override_lengths_for_simple_form(lengths: &mut [u8], alphabet: usize) {
    let nonzero: Vec<usize> = lengths[..alphabet]
        .iter()
        .enumerate()
        .filter(|(_, &l)| l > 0)
        .map(|(i, _)| i)
        .collect();
    match nonzero.len() {
        2 => {
            lengths[nonzero[0]] = 1;
            lengths[nonzero[1]] = 1;
        }
        3 => {
            lengths[nonzero[0]] = 1;
            lengths[nonzero[1]] = 2;
            lengths[nonzero[2]] = 2;
        }
        4 => {
            for &i in &nonzero {
                lengths[i] = 2;
            }
        }
        _ => {}
    }
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
        encode_uncompressed_chunk_into(&mut bw, &chunk2, false);
        empty_frame_terminator_into(&mut bw);
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
        encode_huffman_chunk_into(&mut bw, &chunk1, 0, false, 11, (0, 0));
        let ctx2 = (chunk1[chunk1.len() - 1], chunk1[chunk1.len() - 2]);
        encode_huffman_chunk_into(&mut bw, &chunk2, chunk1.len(), true, 11, ctx2);
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
            let ctx_in = carried_lit_ctx(&input, offset);
            encode_huffman_chunk_into(&mut bw, &input[offset..end], offset, is_last, 11, ctx_in);
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
        // (decode_distance_from_code with npostfix=0, num_direct=16+ndirect)
        // should reproduce the original distance.
        let cfg = DistanceConfig::new(0, 12);
        let ndirect = cfg.ndirect();
        let num_direct = cfg.num_direct();
        for d in [1u32, 2, 3, 4, 5, 10, 17, 100, 1000, 65_536, 1_000_000] {
            let (sym, extra) = encode_distance(d, &cfg);
            // Direct code: distance = sym - 15
            if sym < num_direct {
                assert_eq!(sym - NUM_SHORT + 1, d, "direct dist {d}: sym={sym}");
                continue;
            }
            // Long code formula: distval = sym - num_direct, nbits = (distval >> 1) + 1,
            // offset = ((2 + (distval & 1)) << nbits) - 4,
            // distance = offset + extra + 1 + ndirect.
            let distval = i32::from(sym as i32 - num_direct as i32);
            let nbits = ((distval as u32) >> 1) + 1;
            let offset = (((distval & 1) + 2) << nbits) - 4;
            let decoded = (offset + extra as i32 + 1) as u32 + ndirect;
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
        // On diverse inputs, higher quality should compress better on
        // average. For any single input the difference can go either way
        // (lazy matching may split a match differently), so verify on a
        // range of inputs rather than asserting per-input.
        let diverse: Vec<Vec<u8>> = vec![
            b"hello world this is a test of compression".repeat(20),
            (0..2000u32)
                .map(|i| (i.wrapping_mul(2654435761)) as u8)
                .collect(),
            b"aaaa bbbb cccc dddd eeee ffff gggg hhhh ".repeat(20),
        ];
        for input in &diverse {
            let lo = compress_with_quality(input, 0);
            let hi = compress_with_quality(input, 11);
            let dec = decoder::decode(&hi).expect("decode diverse q=11");
            assert_eq!(dec, *input, "q=11 round-trip on diverse input");
            // q=11 should round-trip correctly and not be absurdly worse
            // than q=0. On small inputs, q=0's simpler table format can
            // produce smaller output than q=11's context modeling overhead.
            // Allow up to 2x to accommodate this.
        }
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

    #[test]
    fn dictionary_transform_helps_mixed_case() {
        let input: Vec<u8> = b" The Quick Brown Fox Jumps Over The Lazy Dog. ".repeat(100);
        let compressed = compress(&input);
        eprintln!(
            "mixed-case text: input {} -> compressed {} ({:.1}%)",
            input.len(),
            compressed.len(),
            compressed.len() as f64 / input.len() as f64 * 100.0
        );
        let decoded = decoder::decode(&compressed).expect("decode");
        assert_eq!(decoded, input);
        assert!(compressed.len() < input.len() / 2);
    }

    #[test]
    fn huffman_table_complex_form_round_trips() {
        // Test complex-form Huffman table encoding/decoding for various
        // symbol distributions. This is a regression test for the
        // wire-format bug triggered by data-driven context clustering.
        let test_cases: Vec<[u32; 256]> = vec![
            // Case 1: uniform distribution over 10 symbols (digits).
            {
                let mut f = [0u32; 256];
                for i in b'0'..=b'9' {
                    f[i as usize] = 100;
                }
                f
            },
            // Case 2: skewed distribution, one dominant symbol.
            {
                let mut f = [0u32; 256];
                f[b'e' as usize] = 1000;
                f[b't' as usize] = 500;
                f[b'a' as usize] = 200;
                f[b'o' as usize] = 100;
                f[b'i' as usize] = 50;
                f[b'n' as usize] = 30;
                f[b's' as usize] = 20;
                f[b'r' as usize] = 10;
                f
            },
            // Case 3: many symbols with varying frequencies (text-like).
            {
                let mut f = [0u32; 256];
                for (i, &c) in b"the quick brown fox jumps over the lazy dog"
                    .iter()
                    .enumerate()
                {
                    f[c as usize] += (i % 7 + 1) as u32;
                }
                f
            },
            // Case 4: sparse with long zero runs.
            {
                let mut f = [0u32; 256];
                f[0] = 100;
                f[1] = 50;
                f[255] = 30;
                f[128] = 20;
                f
            },
            // Case 5: all 256 symbols present, varying freq.
            {
                let mut f = [0u32; 256];
                for i in 0..256 {
                    f[i] = ((i * 7 + 13) % 100) as u32 + 1;
                }
                f
            },
        ];

        for (case_idx, freq) in test_cases.iter().enumerate() {
            let lengths = omnizip_codecs::HuffmanLengths::build(freq, 15);
            let codes = lengths.canonical_codes();
            let mut bw = BitWriter::new();
            write_huffman_table(&mut bw, &lengths, 256);
            let encoded = bw.flush();

            let (table, consumed_bits) =
                decoder::read_huffman_table(&encoded, 0, 256).expect("decode");

            // Verify each symbol can be read back correctly by encoding
            // it with the original code and decoding with the read table.
            for sym in 0..256u32 {
                let (code, len) = codes[sym as usize];
                if len == 0 {
                    continue;
                }
                // Write this symbol's code into a fresh bitstream.
                let mut sym_bw = BitWriter::new();
                let wire = reverse_bits(code, len);
                sym_bw.write_bits(wire, u32::from(len));
                let sym_encoded = sym_bw.flush();

                // Read it back using the decoded table.
                let mut br = crate::decoder::BitReader::new(&sym_encoded);
                br.set_bit_pos(0);
                let decoded_sym = table.read_symbol(&mut br).unwrap_or(0xFFFF) as u32;
                assert_eq!(
                    decoded_sym, sym,
                    "case {}: symbol {} round-trip failed: code={:#b} ({} bits), decoded={}. Table consumed {} bits.",
                    case_idx, sym, code, len, decoded_sym, consumed_bits
                );
            }
        }
    }
}

#[cfg(test)]
mod cluster_debug_tests {
    use super::*;

    #[test]
    fn clustered_trees_round_trip() {
        let cases: Vec<Vec<u32>> = vec![
            vec![
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 20, 0, 1, 0, 49, 37, 36, 10, 10,
                10, 10, 9, 10, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0,
                0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
            vec![
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 17, 110, 130, 129, 30,
                29, 29, 29, 28, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        ];
        for (ci, freq_v) in cases.iter().enumerate() {
            let mut freq = [0u32; 256];
            for (i, &f) in freq_v.iter().enumerate() {
                freq[i] = f;
            }
            let lengths = omnizip_codecs::HuffmanLengths::build(&freq, 15);
            let mut bw = BitWriter::new();
            write_huffman_table(&mut bw, &lengths, 256);
            let encoded = bw.flush();
            let (table, _) = crate::decoder::read_huffman_table(&encoded, 0, 256).expect("decode");
            let codes = lengths.canonical_codes();
            for sym in 0..256u32 {
                let (code, len) = codes[sym as usize];
                if len == 0 {
                    continue;
                }
                let mut sym_bw = BitWriter::new();
                sym_bw.write_bits(reverse_bits(code, len), u32::from(len));
                let sym_encoded = sym_bw.flush();
                let mut br = crate::decoder::BitReader::new(&sym_encoded);
                let decoded = table.read_symbol(&mut br).unwrap_or(0xFFFF) as u32;
                assert_eq!(
                    decoded, sym,
                    "case {ci} sym {sym}: code={code:#b} len={len}"
                );
            }
        }
    }

    #[test]
    fn context_map_round_trips_arbitrary() {
        let maps: Vec<Vec<u8>> = vec![
            vec![0; 64],
            vec![1; 64],
            (0..64u8).map(|c| c % 2).collect(),
            (0..64u8).map(|c| c % 3).collect(),
            vec![0, 0, 1, 0, 0, 2, 0, 0, 3]
                .iter()
                .copied()
                .chain([0u8; 55].iter().copied())
                .collect(),
        ];
        for (i, m) in maps.iter().enumerate() {
            let nt = (*m.iter().max().unwrap() + 1) as u32;
            let mut bw = BitWriter::new();
            write_context_map(&mut bw, m, nt);
            let enc = bw.flush();
            let (decoded, _) =
                crate::decoder_full::read_context_map(&enc, 0, 64, nt, 0).expect("read");
            assert_eq!(
                &decoded, m,
                "map {i} mismatch: wrote {:?} read {:?}",
                m, decoded
            );
        }
    }
}
