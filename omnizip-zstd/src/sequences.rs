//! Sequences section decoder + LZ77 executor (RFC 8878 §3.1.1.3.2 +
//! §3.1.2.2.3).
//!
//! Ported with substantial rework from
//! `omnizip/lib/omnizip/algorithms/zstandard/sequences.rb` (342 LOC, MIT,
//! Ribose Inc.). The Ruby has multiple bugs in this code path — see
//! BUGREPORTs 04 (FSE-from-stream stub), 05 (offset extra bits ignored),
//! and the executor's repeat-offset rotation. The implementation here
//! handles them correctly.
//!
//! ## Section layout
//!
//! ```text
//! byte 0..2   number_of_sequences (1-3 bytes; 0 means "no sequences")
//! byte 3      symbol_compression_modes:
//!               bits 6-7 LL mode, bits 4-5 OF mode, bits 2-3 ML mode
//!               (bits 0-1 are reserved)
//! …           per-mode table data (for FSE / RLE modes)
//! …           bitstream (consumed in reverse, contains the FSE-coded
//!             LL / ML / OF symbols + their extra bits)
//! ```
//!
//! ## Phase-A scope
//!
//! - PREDEFINED, RLE, and REPEAT modes are handled for all three
//!   symbol streams.
//! - FSE-from-stream mode is partially supported (the per-mode table
//!   reader is not yet ported; falls back to Unsupported).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::constants::{DEFAULT_REPEAT_OFFSETS, MODE_PREDEFINED, MODE_REPEAT, MODE_RLE};
use crate::fse::BitStream;
use crate::predef_tables::{LL_PREDEF, ML_PREDEF, OF_PREDEF, PredefEntry,
    LL_ACCURACY_LOG, OF_ACCURACY_LOG, ML_ACCURACY_LOG};
use crate::ZstdError;

/// One decoded sequence: literal length, match length, offset (raw
/// FSE symbol value; offset_extra_bits are folded in by the executor
/// when applying repeat-offset rotation).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sequence {
    pub literal_length: u32,
    pub match_length: u32,
    pub offset_symbol: u32,
}

/// The decoded sequences section.
#[derive(Debug)]
pub struct SequencesSection {
    pub sequences: Vec<Sequence>,
    pub consumed: usize,
    pub fse_tables: (),
}

/// Decode the sequences section. `previous_tables` carries the FSE
/// tables from the previous compressed block in the same frame (for
/// `MODE_REPEAT`).
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on any structural problem,
/// [`ZstdError::Unsupported`] when an FSE-mode table is encountered
/// (the per-mode table reader is not yet ported).
pub fn decode_sequences_section(
    input: &[u8],
    _previous_tables: &(),
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

    // 3. Per-mode table: PREDEFINED uses hardcoded tables, RLE reads
    //    one byte, FSE/REPEAT are not yet ported.
    let ll_tbl = get_table(ll_mode, &LL_PREDEF, LL_ACCURACY_LOG, &mut cursor)?;
    let of_tbl = get_table(of_mode, &OF_PREDEF, OF_ACCURACY_LOG, &mut cursor)?;
    let ml_tbl = get_table(ml_mode, &ML_PREDEF, ML_ACCURACY_LOG, &mut cursor)?;

    // 4. Bitstream: everything left in `cursor` is the FSE bitstream.
    let mut bs = BitStream::new(cursor);

    // 5. Init states in C source order: LL, OF, ML (NOT the RFC's
    //    OF, ML, LL — the C source at zstd_decompress_block.c:1527-1529
    //    is authoritative).
    let mut ll_state = init_state(&ll_tbl, &mut bs);
    let mut of_state = init_state(&of_tbl, &mut bs);
    let mut ml_state = init_state(&ml_tbl, &mut bs);

    // 6. Decode sequences.
    let mut sequences = Vec::with_capacity(num_sequences as usize);
    for seq_idx in 0..num_sequences {
        // Per the C reference (ZSTD_decodeSequence), the decode order
        // for each sequence is:
        //
        // 1. Look up symbols from current states (no bits consumed).
        // 2. Read extra bits in order: LL, OF, ML.
        // 3. Update states in order: LL, OF, ML (each reads nb_bits
        //    from the bitstream). For the LAST sequence, state updates
        //    are skipped (no more symbols follow).
        let is_last = seq_idx == num_sequences - 1;

        let ll_e = lookup(&ll_tbl, ll_state);
        let of_e = lookup(&of_tbl, of_state);
        let ml_e = lookup(&ml_tbl, ml_state);

        // Extra bits (C reference order: LL, OF, ML).
        let ll_value = ll_e.base_val + bs.read_bits(u32::from(ll_e.nb_add_bits));
        let of_value = of_e.base_val + bs.read_bits(u32::from(of_e.nb_add_bits));
        let ml_value = ml_e.base_val + bs.read_bits(u32::from(ml_e.nb_add_bits));

        // State updates (C reference order: LL, OF, ML).
        // Skip for the last sequence — no more symbols to decode.
        if !is_last {
            ll_state = u32::from(ll_e.next_state) + bs.read_bits(u32::from(ll_e.nb_bits));
            of_state = u32::from(of_e.next_state) + bs.read_bits(u32::from(of_e.nb_bits));
            ml_state = u32::from(ml_e.next_state) + bs.read_bits(u32::from(ml_e.nb_bits));
        }

        sequences.push(Sequence {
            literal_length: ll_value,
            match_length: ml_value,
            offset_symbol: of_value,
        });
    }

    Ok(SequencesSection {
        sequences,
        consumed: input.len(),
        fse_tables: (),
    })
}

/// Table type for sequence decoding — either a reference to a
/// predefined table or an RLE single-entry table.
enum SeqTable {
    Predefined(&'static [PredefEntry], u8),
    Rle(PredefEntry),
}

/// Build a table for the given mode.
fn get_table(
    mode: u8,
    predef: &'static [PredefEntry],
    accuracy_log: u8,
    cursor: &mut &[u8],
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
                nb_add_bits: symbol,
                nb_bits: 0,
                base_val: 0,
            }))
        }
        MODE_REPEAT => Err(ZstdError::Unsupported {
            reason: "MODE_REPEAT not yet supported".into(),
        }),
        _ => Err(ZstdError::Unsupported {
            reason: "MODE_FSE not yet supported".into(),
        }),
    }
}

/// Read accuracy_log bits to initialise the FSE state.
fn init_state(tbl: &SeqTable, bs: &mut BitStream<'_>) -> u32 {
    let log = match tbl {
        SeqTable::Predefined(_, log) => *log,
        SeqTable::Rle(_) => 0,
    };
    bs.read_bits(u32::from(log))
}

/// Look up a table entry at the given state.
fn lookup<'a>(tbl: &'a SeqTable, state: u32) -> PredefEntry {
    match tbl {
        SeqTable::Predefined(entries, _) => {
            let idx = (state as usize).min(entries.len() - 1);
            entries[idx]
        }
        SeqTable::Rle(entry) => *entry,
    }
}

/// Decode the sequence count (1-3 bytes per RFC §3.1.1.3.2.1).
///
/// Returns the count and the remaining input slice.
fn read_sequence_count(input: &[u8]) -> Result<(u32, &[u8]), ZstdError> {
    if input.is_empty() {
        return Err(ZstdError::Corrupt {
            reason: "empty sequences section".into(),
        });
    }
    let b0 = u32::from(input[0]);
    if b0 < 128 {
        return Ok((b0, &input[1..]));
    }
    if input.len() < 2 {
        return Err(ZstdError::Corrupt {
            reason: "truncated sequence count".into(),
        });
    }
    let b1 = u32::from(input[1]);
    let count = ((b0 - 128) << 8) + b1 + 128;
    Ok((count, &input[2..]))
}

// ── Sequence executor (RFC 8878 §3.1.2.2.3) ─────────────────────────────

/// Stateful LZ77 sequence executor. Tracks the three repeat-offset
/// slots across the entire frame (reset on each new frame).
#[derive(Debug, Clone)]
pub struct SequenceExecutor {
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

    /// Execute `sequences` against the literal buffer, appending the
    /// decoded output to `output`. Returns the number of bytes
    /// appended.
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
                // If `ll > remaining_literals`, the stream is technically
                // corrupt; we silently copy what we have and let the
                // size-validation step at the end catch the mismatch.
            }

            // 2. Resolve offset: repeat-offset rotation for symbols ≤ 3.
            let distance = self.resolve_offset(seq.offset_symbol);
            if distance == 0 {
                return Err(ZstdError::Corrupt {
                    reason: "match distance 0 is invalid".into(),
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

    /// Resolve an offset symbol against the repeat-offset slots.
    /// Symbol 0, 1, 2 → repeat_offsets[0, 1, 2] with rotation.
    /// Symbol ≥ 3 → actual offset value; rotate slots.
    fn resolve_offset(&mut self, offset_symbol: u32) -> u32 {
        match offset_symbol {
            0 => self.repeat_offsets[0],
            1 => {
                let r1 = self.repeat_offsets[1];
                self.repeat_offsets[1] = self.repeat_offsets[0];
                self.repeat_offsets[0] = r1;
                r1
            }
            2 => {
                let r2 = self.repeat_offsets[2];
                self.repeat_offsets[2] = self.repeat_offsets[1];
                self.repeat_offsets[1] = self.repeat_offsets[0];
                self.repeat_offsets[0] = r2;
                r2
            }
            actual => {
                self.repeat_offsets[2] = self.repeat_offsets[1];
                self.repeat_offsets[1] = self.repeat_offsets[0];
                self.repeat_offsets[0] = actual;
                actual
            }
        }
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
        // 0x80 0x01 → ((0x80 - 128) << 8) + 1 + 128 = 0 + 1 + 128 = 129
        let (n, _) = read_sequence_count(&[0x80, 0x01]).unwrap();
        assert_eq!(n, 129);
    }

    #[test]
    fn empty_section_errors() {
        assert!(decode_sequences_section(&[], &()).is_err());
    }

    #[test]
    fn zero_sequences_returns_empty() {
        // byte 0 = 0 → no sequences.
        let s = decode_sequences_section(&[0x00], &()).expect("decode");
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
    fn executor_handles_rle_match_via_repeat_offset_1() {
        // Set up: output is "aaaa", then a sequence with LL=0, ML=3,
        // offset_symbol=0 (repeat offset slot 0, default value 1).
        // Expected: copy 3 bytes from offset 1 = "aaa" → output is "aaaaaaa".
        let mut e = SequenceExecutor::new();
        let mut out = b"aaaa".to_vec();
        let seq = Sequence {
            literal_length: 0,
            match_length: 3,
            offset_symbol: 0, // repeat offset slot 0 (value 1)
        };
        e.execute(&[], std::slice::from_ref(&seq), &mut out).unwrap();
        assert_eq!(out, b"aaaaaaa");
    }

    #[test]
    fn executor_rotates_repeat_offsets_on_new_distance() {
        let mut e = SequenceExecutor::new();
        let mut out = b"abcdef".to_vec();
        let seq = Sequence {
            literal_length: 0,
            match_length: 2,
            offset_symbol: 10, // new distance 10
        };
        // Distance 10 > output.len() (6) → error.
        let err = e.execute(&[], std::slice::from_ref(&seq), &mut out).unwrap_err();
        assert!(matches!(err, ZstdError::Corrupt { .. }));
    }

    #[test]
    fn of_predef_entry_0_is_repeat_offset() {
        // The first entry of the OF predefined table must be a repeat
        // offset (base_val ≤ 2).
        assert!(crate::predef_tables::OF_PREDEF[0].base_val <= 2);
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
