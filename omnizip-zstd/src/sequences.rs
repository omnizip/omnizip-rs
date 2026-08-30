//! Sequences section decoder + LZ77 executor (RFC 8878 §3.1.1.3.2 +
//! §3.1.2.2.3).
//!
//! Verified against the C reference `ZSTD_decodeSequence` in
//! `~/src/external/zstd/lib/decompress/zstd_decompress_block.c:1235-1447`.
//! Key invariants the C source imposes:
//!
//! - **State init order**: LL, OF, ML (C lines 1527–1529).
//! - **Per-sequence decode**: look up LL/OF/ML symbols (no bits consumed),
//!   then read extra bits in order **OF, ML, LL** (C lines 1393, 1417,
//!   1427), then update FSE states in order **LL, ML, OF** (C lines
//!   1437–1440). State updates are skipped for the last sequence.
//! - **Offset resolution**: depends on `ofBits` (the table entry's
//!   `nb_add_bits`) and `ll0 = (ll_base == 0)`:
//!   - `ofBits > 1`: `offset = ofBase + read(ofBits)`; rotate normally.
//!   - `ofBits == 0`: `offset = prevOffset[ll0]`; conditional slot 1
//!     shuffle when `ll0 == 1`.
//!   - `ofBits == 1`: `offset = ofBase + ll0 + read(1)`; complex
//!     repeat-offset rotation with `prevOffset[0] - 1` special case.
//!
//! ## Section layout
//!
//! ```text
//! byte 0..2   number_of_sequences (1-3 bytes; 0 means "no sequences")
//! byte 3      symbol_compression_modes:
//!               bits 6-7 LL mode, bits 4-5 OF mode, bits 2-3 ML mode
//! …           per-mode table data (for FSE / RLE modes)
//! …           bitstream (consumed in reverse, contains the FSE-coded
//!             LL / ML / OF symbols + their extra bits)
//! ```

#![forbid(unsafe_code)]

use crate::constants::{DEFAULT_REPEAT_OFFSETS, MODE_FSE, MODE_PREDEFINED, MODE_REPEAT, MODE_RLE};
use crate::fse::BitStream;
use crate::predef_tables::{
    PredefEntry, LL_ACCURACY_LOG, LL_PREDEF, ML_ACCURACY_LOG, ML_PREDEF, OF_ACCURACY_LOG, OF_PREDEF,
};
use crate::ZstdError;

/// Code-to-base-value and code-to-extra-bits lookup tables for FSE mode.
const LL_BASE: [u32; 36] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24, 28, 32, 40, 48, 64,
    128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
];
const LL_BITS: [u8; 36] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 4, 6, 7, 8, 9, 10, 11,
    12, 13, 14, 15, 16,
];
const ML_BASE: [u32; 53] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
    28, 29, 30, 31, 32, 33, 34, 35, 37, 39, 41, 43, 47, 51, 59, 67, 83, 99, 131, 259, 515, 1027,
    2051, 4099, 8195, 16387, 32771, 65539,
];
const ML_BITS: [u8; 53] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    1, 1, 1, 1, 2, 2, 3, 3, 4, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
];
// C reference (zstd_decompress_internal.h): OF_base / OF_bits.
// Codes 0-1 are the repeat-offset specials (handled in resolve_offset);
// codes >= 2 carry real distances: distance = OF_BASE[code] + extra.
const OF_BASE: [u32; 32] = [
    0, 1, 1, 5, 13, 29, 61, 125, 253, 509, 1021, 2045, 4093, 8189, 16381, 32765, 65533, 131069,
    262141, 524285, 1048573, 2097149, 4194301, 8388605, 16777213, 33554429, 67108861, 134217725,
    268435453, 536870909, 1073741821, 2147483645,
];
const OF_BITS: [u8; 32] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31,
];

/// One decoded sequence. `offset` is the resolved byte distance (the
/// repeat-offset rotation has already been applied by the decoder).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sequence {
    pub literal_length: u32,
    pub match_length: u32,
    pub offset: u32,
}

/// The decoded sequences section. `sequences[i].offset` carries the
/// resolved byte distance; the executor applies them directly.
#[derive(Debug)]
pub struct SequencesSection {
    pub sequences: Vec<Sequence>,
    pub consumed: usize,
    pub fse_tables: (),
}

/// Decode the sequences section. `previous_tables` carries the FSE
/// tables from the previous compressed block in the same frame (for
/// `MODE_REPEAT`). `executor` supplies the repeat-offset state and is
/// updated in place as each sequence resolves its offset.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on any structural problem,
/// [`ZstdError::Unsupported`] when an FSE-mode table is encountered
///
pub fn decode_sequences_section(
    input: &[u8],
    _previous_tables: &(),
    executor: &mut SequenceExecutor,
) -> Result<SequencesSection, ZstdError> {
    if input.is_empty() {
        return Err(ZstdError::Corrupt {
            reason: "empty sequences section".into(),
        });
    }

    // 1. Sequence count (1-3 bytes).
    let (num_sequences, after_count) = read_sequence_count(input)?;

    if num_sequences == 0 {
        return Ok(SequencesSection {
            sequences: Vec::new(),
            consumed: input.len() - after_count.len(),
            fse_tables: (),
        });
    }

    // 2. Symbol-compression modes (1 byte).
    if after_count.is_empty() {
        return Err(ZstdError::Corrupt {
            reason: "truncated sequences section: missing modes byte".into(),
        });
    }
    let modes = after_count[0];
    let ll_mode = (modes >> 6) & 0x03;
    let of_mode = (modes >> 4) & 0x03;
    let ml_mode = (modes >> 2) & 0x03;
    let mut cursor = &after_count[1..];

    // 3. Per-mode table.
    let ll_tbl = get_table(
        ll_mode,
        &LL_PREDEF,
        LL_ACCURACY_LOG,
        &mut cursor,
        &LL_BASE,
        &LL_BITS,
    )?;
    let of_tbl = get_table(
        of_mode,
        &OF_PREDEF,
        OF_ACCURACY_LOG,
        &mut cursor,
        &OF_BASE,
        &OF_BITS,
    )?;
    let ml_tbl = get_table(
        ml_mode,
        &ML_PREDEF,
        ML_ACCURACY_LOG,
        &mut cursor,
        &ML_BASE,
        &ML_BITS,
    )?;

    // 4. Bitstream: everything left in `cursor` is the FSE bitstream.
    let mut bs = BitStream::new(cursor);

    // 5. Init FSE states in C source order: LL, OF, ML
    //    (zstd_decompress_block.c:1527-1529). Each init reads bits
    //    and reloads the container, matching ZSTD_initFseState.
    let mut ll_state = init_state(&ll_tbl, &mut bs);
    bs.reload();
    let mut of_state = init_state(&of_tbl, &mut bs);
    bs.reload();
    let mut ml_state = init_state(&ml_tbl, &mut bs);
    bs.reload();

    // 6. Decode sequences following ZSTD_decodeSequence exactly.
    let mut sequences = Vec::with_capacity(num_sequences as usize);
    for seq_idx in 0..num_sequences {
        let is_last = seq_idx == num_sequences - 1;

        // (a) Symbol lookups — no bits consumed.
        let ll_e = lookup(&ll_tbl, ll_state);
        let of_e = lookup(&of_tbl, of_state);
        let ml_e = lookup(&ml_tbl, ml_state);

        // (b) Resolve the offset using the C reference's ofBits/ll0 logic.
        //     This may read 0, 1, or `nb_add_bits` extra bits from the
        //     bitstream and updates the executor's repeat-offset slots.
        let ll0 = ll_e.base_val == 0;
        let offset = executor.resolve_offset(of_e.base_val, of_e.nb_add_bits, ll0, &mut bs);

        // (c) Read ML extra bits, then LL extra bits (C: mlBits before llBits).
        let match_length = ml_e.base_val + bs.read_bits(u32::from(ml_e.nb_add_bits));
        let literal_length = ll_e.base_val + bs.read_bits(u32::from(ll_e.nb_add_bits));

        // (d) State updates — order LL, ML, OF. Skipped for the last
        //     sequence (no more symbols to decode).
        if !is_last {
            ll_state = u32::from(ll_e.next_state) + bs.read_bits(u32::from(ll_e.nb_bits));
            ml_state = u32::from(ml_e.next_state) + bs.read_bits(u32::from(ml_e.nb_bits));
            of_state = u32::from(of_e.next_state) + bs.read_bits(u32::from(of_e.nb_bits));
            // Reload the bitstream container after each sequence, matching
            // C's `BIT_reloadDStream` at the end of ZSTD_decodeSequence.
            bs.reload();
        }

        sequences.push(Sequence {
            literal_length,
            match_length,
            offset,
        });
    }

    if std::env::var("ZSTD_SEQ_STATS").is_ok() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        static LITS: AtomicU64 = AtomicU64::new(0);
        static MBYTES: AtomicU64 = AtomicU64::new(0);
        static MAXML: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(u64::try_from(sequences.len()).unwrap_or(0), Ordering::Relaxed);
        for s2 in &sequences {
            LITS.fetch_add(u64::from(s2.literal_length), Ordering::Relaxed);
            MBYTES.fetch_add(u64::from(s2.match_length), Ordering::Relaxed);
            MAXML.fetch_max(u64::from(s2.match_length), Ordering::Relaxed);
        }
        eprintln!(
            "SEQ_STATS total: seqs={} lits={} mbytes={} max_ml={}",
            N.load(Ordering::Relaxed),
            LITS.load(Ordering::Relaxed),
            MBYTES.load(Ordering::Relaxed),
            MAXML.load(Ordering::Relaxed)
        );
    }

    Ok(SequencesSection {
        sequences,
        consumed: input.len(),
        fse_tables: (),
    })
}

/// Table type for sequence decoding — either a reference to a
/// predefined table, an RLE single-entry table, or a dynamically
/// built FSE table.
enum SeqTable {
    Predefined(&'static [PredefEntry], u8),
    Owned(Vec<PredefEntry>, u8),
    Rle(PredefEntry),
}

/// Build a table for the given mode.
fn get_table(
    mode: u8,
    predef: &'static [PredefEntry],
    accuracy_log: u8,
    cursor: &mut &[u8],
    base_table: &[u32],
    bits_table: &[u8],
) -> Result<SeqTable, ZstdError> {
    match mode {
        MODE_PREDEFINED => Ok(SeqTable::Predefined(predef, accuracy_log)),
        MODE_RLE => {
            if cursor.is_empty() {
                return Err(ZstdError::Corrupt {
                    reason: "RLE mode: missing symbol byte".into(),
                });
            }
            let symbol = cursor[0];
            *cursor = &cursor[1..];
            Ok(SeqTable::Rle(PredefEntry {
                next_state: 0,
                nb_add_bits: bits_table[symbol as usize],
                nb_bits: 0,
                base_val: base_table[symbol as usize],
            }))
        }
        MODE_FSE => {
            // Read the custom FSE table from the bitstream.
            let (dtable, consumed) = crate::fse::read_fse_table(cursor)?;
            *cursor = &cursor[consumed..];

            // Convert the generic FSE DTable entries into ZSTD
            // sequence symbol entries. For each state, the FSE
            // `symbol` maps to a code whose base/bits come from
            // the code tables.
            let mut entries = Vec::with_capacity(dtable.size());
            for i in 0..dtable.size() {
                let fs = dtable.state(i);
                let sym = usize::from(fs.symbol);
                let base_val = if sym < base_table.len() {
                    base_table[sym]
                } else {
                    0
                };
                let nb_add_bits = if sym < bits_table.len() {
                    bits_table[sym]
                } else {
                    0
                };
                entries.push(PredefEntry {
                    next_state: fs.baseline as u16,
                    nb_add_bits,
                    nb_bits: fs.num_bits,
                    base_val,
                });
            }
            Ok(SeqTable::Owned(entries, dtable.accuracy_log()))
        }
        MODE_REPEAT => Err(ZstdError::Unsupported {
            reason: "MODE_REPEAT requires prior table state".into(),
        }),
        _ => Err(ZstdError::Corrupt {
            reason: format!("invalid sequence table mode: {mode}"),
        }),
    }
}

/// Read `accuracy_log` bits to initialise the FSE state.
fn init_state(tbl: &SeqTable, bs: &mut BitStream<'_>) -> u32 {
    let log = match tbl {
        SeqTable::Predefined(_, log) => *log,
        SeqTable::Owned(_, log) => *log,
        SeqTable::Rle(_) => 0,
    };
    bs.read_bits(u32::from(log))
}

/// Look up a table entry at the given state.
fn lookup(tbl: &SeqTable, state: u32) -> PredefEntry {
    match tbl {
        SeqTable::Predefined(entries, _) => {
            let idx = (state as usize).min(entries.len() - 1);
            entries[idx]
        }
        SeqTable::Owned(entries, _) => {
            let idx = (state as usize).min(entries.len() - 1);
            entries[idx]
        }
        SeqTable::Rle(entry) => *entry,
    }
}

/// Decode the sequence count (1-3 bytes per RFC §3.1.1.3.2.1).
///
/// Matches the C reference (`zstd_decompress_block.c)`:
/// - `b0 < 0x80` → 1-byte: `nbSeq = b0`
/// - `b0 == 0xFF` → 3-byte: `nbSeq = LE16(b1, b2) + LONGNBSEQ (0x7F00)`
/// - otherwise → 2-byte: `nbSeq = ((b0 - 0x80) << 8) + b1`
fn read_sequence_count(input: &[u8]) -> Result<(u32, &[u8]), ZstdError> {
    if input.is_empty() {
        return Err(ZstdError::Corrupt {
            reason: "empty sequences section".into(),
        });
    }
    let b0 = u32::from(input[0]);
    if b0 < 0x80 {
        return Ok((b0, &input[1..]));
    }
    if b0 == 0xFF {
        if input.len() < 3 {
            return Err(ZstdError::Corrupt {
                reason: "truncated 3-byte sequence count".into(),
            });
        }
        let n = u32::from(u16::from_le_bytes([input[1], input[2]])) + 0x7F00;
        return Ok((n, &input[3..]));
    }
    if input.len() < 2 {
        return Err(ZstdError::Corrupt {
            reason: "truncated 2-byte sequence count".into(),
        });
    }
    let b1 = u32::from(input[1]);
    Ok((((b0 - 0x80) << 8) + b1, &input[2..]))
}

// ── Sequence executor (RFC 8878 §3.1.2.2.3) ─────────────────────────────

/// Stateful LZ77 sequence executor. Tracks the three repeat-offset
/// slots across the entire frame (reset on each new frame).
#[derive(Debug, Clone)]
pub struct SequenceExecutor {
    /// Repeat-offset slots, indexed as `prevOffset[0..=2]` in the C
    /// reference. Defaults to `[1, 4, 8]` at frame start.
    pub repeat_offsets: [u32; 3],
}

impl Default for SequenceExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl SequenceExecutor {
    /// Construct an executor with the ZSTD default repeat offsets
    /// `[1, 4, 8]`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            repeat_offsets: DEFAULT_REPEAT_OFFSETS,
        }
    }

    /// Reset to the default repeat offsets. Called at frame start.
    pub fn reset(&mut self) {
        self.repeat_offsets = DEFAULT_REPEAT_OFFSETS;
    }

    /// Resolve an offset using the C reference's `ofBits`/`ll0` logic.
    /// Updates `self.repeat_offsets` in place.
    ///
    /// - `of_base`: the FSE table entry's `base_val`.
    /// - `of_bits`: the FSE table entry's `nb_add_bits`.
    /// - `ll_base_is_zero`: `true` iff the LL base value is 0 (i.e. the
    ///   sequence emits no literals before the match).
    /// - `bs`: the FSE bitstream, used when `of_bits > 0`.
    #[allow(clippy::similar_names)]
    pub fn resolve_offset(
        &mut self,
        of_base: u32,
        of_bits: u8,
        ll_base_is_zero: bool,
        bs: &mut BitStream<'_>,
    ) -> u32 {
        let ll0 = u32::from(ll_base_is_zero);
        let prev = &mut self.repeat_offsets;

        if of_bits > 1 {
            // C: offset = ofBase + read(ofBits); rotate normally.
            let offset = of_base + bs.read_bits(u32::from(of_bits));
            prev[2] = prev[1];
            prev[1] = prev[0];
            prev[0] = offset;
            offset
        } else if of_bits == 0 {
            // C: offset = prevOffset[ll0]; conditional slot-1 shuffle.
            let offset = prev[ll0 as usize];
            if ll0 == 1 {
                prev[1] = prev[0];
                prev[0] = offset;
            }
            offset
        } else {
            // of_bits == 1: offset = ofBase + ll0 + read(1).
            let mut offset = of_base + ll0 + bs.read_bits(1);
            let mut temp = match offset {
                1 => prev[1],
                3 => prev[0].saturating_sub(1),
                _ if offset >= 2 => prev[2],
                _ => prev[0],
            };
            if temp == 0 {
                // C: `temp -= !temp` forces 0 → underflow → caught by executor.
                temp = u32::MAX;
            }
            if offset != 1 {
                prev[2] = prev[1];
            }
            prev[1] = prev[0];
            prev[0] = temp;
            offset = temp;
            offset
        }
    }

    /// Execute `sequences` against the literal buffer, appending the
    /// decoded output to `output`. Returns the number of bytes appended.
    ///
    /// Each `Sequence.offset` is already the resolved byte distance
    /// (the decoder applied repeat-offset rotation during decode).
    ///
    /// # Errors
    ///
    /// Returns [`ZstdError::Corrupt`] if a match references an offset
    /// larger than the current output (the stream is corrupt).
    pub fn execute(
        &mut self,
        literals: &[u8],
        sequences: &[Sequence],
        output: &mut Vec<u8>,
    ) -> Result<usize, ZstdError> {
        let start_len = output.len();
        let mut lit_pos = 0usize;

        for seq in sequences {
            // 1. Copy literal_length bytes from literals into output.
            let ll = seq.literal_length as usize;
            if ll > 0 {
                let take = ll.min(literals.len().saturating_sub(lit_pos));
                output.extend_from_slice(&literals[lit_pos..lit_pos + take]);
                lit_pos += take;
            }

            // 2. Validate the resolved offset against the current output.
            let distance = seq.offset;
            if distance == 0 || distance == u32::MAX {
                return Err(ZstdError::Corrupt {
                    reason: format!("invalid match distance {distance}"),
                });
            }
            let distance_us = usize::try_from(distance).map_err(|_| ZstdError::Corrupt {
                reason: format!("match distance {distance} exceeds usize"),
            })?;
            if distance_us > output.len() {
                return Err(ZstdError::Corrupt {
                    reason: format!(
                        "match distance {distance} exceeds output length {}",
                        output.len()
                    ),
                });
            }

            // 3. Copy match_length bytes from `distance` back. May
            //    overlap (RLE-style).
            let ml = seq.match_length as usize;
            let src_start = output.len() - distance_us;
            output.reserve(ml);
            for i in 0..ml {
                let byte = output[src_start + (i % distance_us)];
                output.push(byte);
            }
        }

        // 4. Append remaining literals (the last sequence has no match).
        if lit_pos < literals.len() {
            output.extend_from_slice(&literals[lit_pos..]);
        }

        Ok(output.len() - start_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_sequence_count_one_byte() {
        let (n, rest) = read_sequence_count(&[0x05]).unwrap();
        assert_eq!(n, 5);
        assert!(rest.is_empty());
    }

    #[test]
    fn read_sequence_count_two_byte() {
        // 0x80 0x01 → ((0x80 - 0x80) << 8) + 1 = 0 + 1 = 1
        let (n, _) = read_sequence_count(&[0x80, 0x01]).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn read_sequence_count_zero_two_byte() {
        // 0x80 0x00 → ((0x80 - 0x80) << 8) + 0 = 0 (the zeroSeq_2B case)
        let (n, _) = read_sequence_count(&[0x80, 0x00]).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn read_sequence_count_three_byte() {
        // 0xFF 0x00 0x80 → LE16(0x00, 0x80) + 0x7F00 = 0x8000 + 0x7F00 = 0xFF00 = 65280
        let (n, _) = read_sequence_count(&[0xFF, 0x00, 0x80]).unwrap();
        assert_eq!(n, 0x8000 + 0x7F00);
    }

    #[test]
    fn empty_section_errors() {
        let mut e = SequenceExecutor::new();
        assert!(decode_sequences_section(&[], &(), &mut e).is_err());
    }

    #[test]
    fn zero_sequences_returns_empty() {
        let mut e = SequenceExecutor::new();
        let s = decode_sequences_section(&[0x00], &(), &mut e).expect("decode");
        assert!(s.sequences.is_empty());
    }

    #[test]
    fn executor_default_repeat_offsets() {
        let e = SequenceExecutor::new();
        assert_eq!(e.repeat_offsets, [1, 4, 8]);
    }

    #[test]
    fn executor_copies_literals_only_when_no_sequences() {
        let mut e = SequenceExecutor::new();
        let mut out = Vec::new();
        let n = e.execute(b"abcde", &[], &mut out).unwrap();
        assert_eq!(n, 5);
        assert_eq!(out, b"abcde");
    }

    #[test]
    fn resolve_offset_ofbits_zero_uses_prev_offset_zero() {
        // Default repeat offsets [1, 4, 8]. With ll_base_is_zero=false
        // (ll0=0), of_bits=0 → offset = prev[0] = 1.
        let mut e = SequenceExecutor::new();
        let mut bs = BitStream::new(&[]);
        let off = e.resolve_offset(0, 0, false, &mut bs);
        assert_eq!(off, 1);
        // No rotation when ll0=0.
        assert_eq!(e.repeat_offsets, [1, 4, 8]);
    }

    #[test]
    fn resolve_offset_ofbits_zero_ll0_swaps_slot_0_and_1() {
        // With ll_base_is_zero=true (ll0=1), of_bits=0 → offset = prev[1];
        // then prev[1] = prev[0]; prev[0] = offset.
        let mut e = SequenceExecutor::new();
        let mut bs = BitStream::new(&[]);
        let off = e.resolve_offset(0, 0, true, &mut bs);
        assert_eq!(off, 4); // prev[1]
        assert_eq!(e.repeat_offsets, [4, 1, 8]); // slot 0 ← 4, slot 1 ← old slot 0
    }

    #[test]
    fn resolve_offset_ofbits_two_reads_two_bits_and_rotates() {
        // of_bits=2, of_base=5. Bitstream value 0b11 → offset = 5+3 = 8.
        // After: prev = [8, 1, 4] (rotation pushes slot 0→1, slot 1→2).
        let mut e = SequenceExecutor::new();
        // Two-byte input with high bits set; we'll read 2 bits.
        let mut bs = BitStream::new(&[0xFF, 0xFF]);
        let off = e.resolve_offset(5, 2, false, &mut bs);
        assert_eq!(off, 8);
        assert_eq!(e.repeat_offsets, [8, 1, 4]);
    }

    #[test]
    fn executor_rejects_zero_distance() {
        let mut e = SequenceExecutor::new();
        let mut out: Vec<u8> = Vec::new();
        let seq = Sequence {
            literal_length: 0,
            match_length: 1,
            offset: 0,
        };
        let err = e
            .execute(&[], std::slice::from_ref(&seq), &mut out)
            .unwrap_err();
        assert!(matches!(err, ZstdError::Corrupt { .. }));
    }

    #[test]
    fn of_predef_has_one_zero_addbits_and_one_one_addbits_entry() {
        // C reference: ofBits==0 (pure repeat) appears once, ofBits==1
        // (1-bit special) appears once; all other entries have ofBits>1.
        let zeros = OF_PREDEF.iter().filter(|e| e.nb_add_bits == 0).count();
        let ones = OF_PREDEF.iter().filter(|e| e.nb_add_bits == 1).count();
        assert_eq!(zeros, 1, "exactly one ofBits=0 entry");
        assert_eq!(ones, 1, "exactly one ofBits=1 entry");
    }

    #[test]
    fn ll_predef_entry_0_has_base_val_0() {
        assert_eq!(crate::predef_tables::LL_PREDEF[0].base_val, 0);
    }

    #[test]
    fn ml_predef_entry_0_has_base_val_3() {
        // ML base starts at 3 (MATCH_LEN_MIN).
        assert_eq!(crate::predef_tables::ML_PREDEF[0].base_val, 3);
    }
}
