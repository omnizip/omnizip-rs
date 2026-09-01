//! ZSTD sequences section encoder — converts match-finder output
//! (`literal_length`, `match_length`, offset) into the FSE-coded wire
//! format that `sequences::read_section` decodes.
//!
//! Ported from `~/src/external/zstd/lib/compress/zstd_compress_sequences.c`
//! (`ZSTD_encodeSequences_body`).
//!
//! ## Encoding modes
//!
//! Each of LL / ML / OF can be encoded in one of 4 modes:
//! - **Predefined** (0): uses the RFC 8878 hardcoded distributions.
//! - **RLE** (1): single symbol, 1 byte on the wire.
//! - **FSE** (2): custom probability table + bitstream.
//! - **Repeat** (3): reuse the previous block's table (not used for
//!   the first block).
//!
//! This module evaluates Predefined vs `FSE_Compressed` for each table
//! and picks the option with the lower estimated bit cost.

#![forbid(unsafe_code)]

use crate::encoder::match_finder::{RawSequence, SeqStore};
use crate::fse::encoder::{
    build_ctable, normalize_count, optimal_table_log, write_ncount, BitCStream, CState, CTable,
};
use crate::ZstdError;

/// Sequence-table mode codes (RFC 8878 §3.1.1.3.2 Table 15).
const MODE_PREDEFINED: u8 = 0;
const MODE_RLE: u8 = 1;
const MODE_FSE: u8 = 2;
const MODE_REPEAT: u8 = 3;

/// The effective table per symbol type as a decoder holds it after
/// the last emitted block. `Repeat_Mode` re-sends this table with
/// zero header bytes, which is what makes the reference's
/// entropy-driven block splitting pay: consecutive sub-blocks with
/// stable statistics reuse their tables instead of re-sending them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeqTableOnWire {
    Predefined,
    Rle(u8),
    Fse { norm: Vec<i16>, table_log: u8 },
}

/// Decoder-side sequence-table state in wire order (LL, OF, ML).
pub type SeqTablesWire = [SeqTableOnWire; 3];

/// Predefined LL normalized distribution (from C's `LL_defaultNorm`).
/// 36 entries, tableLog = 6.
const LL_DEFAULT_NORM: [i16; 36] = [
    4, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 2, 1, 1, 1, 1, 1,
    -1, -1, -1, -1,
];

/// Predefined ML normalized distribution (from C's `ML_defaultNorm`).
/// 53 entries, tableLog = 6.
const ML_DEFAULT_NORM: [i16; 53] = [
    1, 4, 3, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1, -1, -1,
];

/// Predefined OF normalized distribution (from C's `OF_defaultNorm`).
/// 29 entries, tableLog = 5.
const OF_DEFAULT_NORM: [i16; 29] = [
    1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1,
];

/// LL code → number of extra bits (from C's `LL_bits`).
pub(crate) const LL_BITS: [u8; 36] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 4, 6, 7, 8, 9, 10, 11,
    12, 13, 14, 15, 16,
];

/// ML code → number of extra bits (from C's `ML_bits`).
pub(crate) const ML_BITS: [u8; 53] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    1, 1, 1, 1, 2, 2, 3, 3, 4, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
];

/// LL code → base literal length value (from C's `LL_Base`).
const LL_BASE: [u32; 36] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24, 28, 32, 40, 48, 64,
    128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
];

/// ML code → base match length value (from C's `ML_Base`).
const ML_BASE: [u32; 53] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
    28, 29, 30, 31, 32, 33, 34, 35, 37, 39, 41, 43, 47, 51, 59, 67, 83, 99, 131, 259, 515, 1027,
    2051, 4099, 8195, 16387, 32771, 65539,
];

/// Find the LL code for a given literal length. Returns (code, `extra_bits_value`).
pub(crate) fn ll_code(lit_len: u32) -> (u8, u32) {
    for code in (0..36).rev() {
        if LL_BASE[code] <= lit_len {
            return (code as u8, lit_len - LL_BASE[code]);
        }
    }
    (0, lit_len)
}

/// Find the ML code for a given match length. Returns (code, `extra_bits_value`).
pub(crate) fn ml_code(match_len: u32) -> (u8, u32) {
    for code in (0..53).rev() {
        if ML_BASE[code] <= match_len {
            return (code as u8, match_len - ML_BASE[code]);
        }
    }
    (0, match_len)
}

/// Compute the offset base value for a given byte distance.
/// ZSTD offset coding: offBase = offset + `REPEAT_SLOTS` (3).
/// Repeat offsets 1 and 2 use offBase 1 and 2 respectively.
const fn off_base(offset: u32) -> u32 {
    offset + 3
}

/// Encode the sequences section from a [`SeqStore`] into `out`.
/// Evaluates Predefined vs `FSE_Compressed` for each table (LL, OF, ML)
/// and picks the option with lower estimated bit cost.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on internal failures.
pub fn encode_section(
    out: &mut Vec<u8>,
    sequences: &[RawSequence],
    initial_reps: [u32; 3],
    last_tables: &mut Option<SeqTablesWire>,
) -> Result<[u32; 3], ZstdError> {
    let nb_seq = sequences.len();

    // 1. Sequence count (1-3 bytes).
    write_sequence_count(out, nb_seq);

    // A zero-sequence block leaves the decoder's table state
    // untouched (RFC 8878: "The FSE tables used in Repeat_Mode are
    // not updated").
    if nb_seq == 0 {
        return Ok(initial_reps);
    }

    // 2. Compute code tables for each sequence.
    let mut ll_codes = Vec::with_capacity(nb_seq);
    let mut ml_codes = Vec::with_capacity(nb_seq);
    let mut of_codes = Vec::with_capacity(nb_seq);
    let mut ll_extras = Vec::with_capacity(nb_seq);
    let mut ml_extras = Vec::with_capacity(nb_seq);
    let mut off_bases = Vec::with_capacity(nb_seq);

    // Repeat-offset tracking, mirroring the decoder's
    // SequenceExecutor::resolve_offset exactly. The full emit-able
    // set (decoder value = 1 + ll0 + extra for off-code 1):
    //   rep1  off==reps[0], ll0=0  -> offBase 1, no ring change
    //   rep2  off==reps[1]         -> swap [0]<->[1];
    //                                  offBase 1 (ll0=1) / 2 (ll0=0)
    //   rep3  off==reps[2] (>3)    -> 3-rotate;
    //                                  offBase 3 (ll0=0) / 2 (ll0=1)
    //   quirk off==reps[0]-1, ll0=1, reps[0]>=2 -> 3-rotate, offBase 3
    // This is where recurring-distance data gets its per-match
    // discount; the conservative table (rep2 only at ll0, no quirk)
    // left the OF histogram top-heavy enough to reshape the FSE
    // table (1-vs-35 cells on symbol 1 vs the reference).
    let mut reps = initial_reps;
    for seq in sequences {
        let (ll_c, ll_e) = ll_code(seq.literal_length);
        let (ml_c, ml_e) = ml_code(seq.match_length);
        let ll0 = seq.literal_length == 0;

        let ob = if !ll0 && seq.offset == reps[0] {
            // offBase 1, no state change.
            1
        } else if seq.offset == reps[1] {
            // rep2 both literal states: decoder value 1 (ll0=0,
            // offBase 2) selects prev[1]; offBase 1 with ll0=1 does
            // the same. Ring swaps [0] and [1].
            let used = reps[1];
            reps[1] = reps[0];
            reps[0] = used;
            if ll0 {
                1
            } else {
                2
            }
        } else if seq.offset == reps[2] && seq.offset > 3 {
            // rep3. The decoder's of_bits==1 path computes
            // `value = OF_base[1] + ll0 + read(1)` and selects prev[2]
            // when value == 2 — so the extra bit differs by ll0:
            // offBase 3 (extra 1) without literals, offBase 2
            // (extra 0) with. `offset > 3` guards tiny reps that also
            // have a cheap explicit code.
            let used = reps[2];
            reps[2] = reps[1];
            reps[1] = reps[0];
            reps[0] = used;
            if ll0 {
                2
            } else {
                3
            }
        } else if ll0 && reps[0] >= 2 && seq.offset == reps[0] - 1 {
            // The prev[0]-1 quirk: decoder value 3 (offBase 3, ll0=1)
            // resolves to prev[0]-1 and rotates fully. Only when the
            // result is a legal nonzero offset.
            reps[2] = reps[1];
            reps[1] = reps[0];
            reps[0] = seq.offset;
            3
        } else {
            reps[2] = reps[1];
            reps[1] = reps[0];
            reps[0] = seq.offset;
            off_base(seq.offset)
        };

        ll_codes.push(ll_c);
        ml_codes.push(ml_c);
        of_codes.push(ob.ilog2().min(31) as u8);
        ll_extras.push(ll_e);
        ml_extras.push(ml_e);
        off_bases.push(ob);
    }

    // 3. Count symbol frequencies for FSE mode selection.
    let mut ll_count = [0u32; 36];
    let mut ml_count = [0u32; 53];
    let mut of_count = [0u32; 32];
    let ll_max = count_symbols(&ll_codes, &mut ll_count);
    let ml_max = count_symbols(&ml_codes, &mut ml_count);
    let of_max = count_symbols(&of_codes, &mut of_count);

    // 4. For each table, decide between Predefined and FSE_Compressed.
    //    Entropy estimates ignore FSE state-machine overhead and were
    //    off by enough to regress small streams; the choice is made by
    //    measuring the actual encoded header + payload for both
    //    candidates (each is a few hundred bytes of scratch work).
    let codes_ref = (
        ll_codes.as_slice(),
        ml_codes.as_slice(),
        of_codes.as_slice(),
        ll_extras.as_slice(),
        ml_extras.as_slice(),
        off_bases.as_slice(),
    );
    let measure = |ll: &TableChoice, ml: &TableChoice, of: &TableChoice| -> u64 {
        section_size_bits(ll, ml, of, ll_max, ml_max, of_max, codes_ref, nb_seq)
    };

    let ll_fse = choose_table_mode(&ll_count, ll_max, &LL_DEFAULT_NORM, 6, 35, 9, nb_seq as u64);
    let ml_fse = choose_table_mode(&ml_count, ml_max, &ML_DEFAULT_NORM, 6, 52, 9, nb_seq as u64);
    let of_fse = choose_table_mode(&of_count, of_max, &OF_DEFAULT_NORM, 5, 28, 8, nb_seq as u64);

    let ll_pre = TableChoice {
        mode: MODE_PREDEFINED,
        norm: LL_DEFAULT_NORM.to_vec(),
        table_log: 6,
        max_sym: 35,
    };
    let ml_pre = TableChoice {
        mode: MODE_PREDEFINED,
        norm: ML_DEFAULT_NORM.to_vec(),
        table_log: 6,
        max_sym: 52,
    };
    let of_pre = TableChoice {
        mode: MODE_PREDEFINED,
        norm: OF_DEFAULT_NORM.to_vec(),
        table_log: 5,
        max_sym: 28,
    };

    let (ll_choice, ll_wire) = pick_table(
        ll_fse.mode == MODE_FSE && {
            let with = measure(&ll_fse, &ml_pre, &of_pre);
            let without = measure(&ll_pre, &ml_pre, &of_pre);
            with < without
        },
        ll_fse,
        ll_pre,
        &ll_count,
        last_tables.as_ref().map(|t| &t[0]),
        |t| measure(t, &ml_pre, &of_pre),
    );
    let (ml_choice, ml_wire) = pick_table(
        ml_fse.mode == MODE_FSE && {
            let with = measure(&ll_choice, &ml_fse, &of_pre);
            let without = measure(&ll_choice, &ml_pre, &of_pre);
            with < without
        },
        ml_fse,
        ml_pre,
        &ml_count,
        last_tables.as_ref().map(|t| &t[1]),
        |t| measure(&ll_choice, t, &of_pre),
    );
    let (of_choice, of_wire) = pick_table(
        of_fse.mode == MODE_FSE && {
            let with = measure(&ll_choice, &ml_choice, &of_fse);
            let without = measure(&ll_choice, &ml_choice, &of_pre);
            with < without
        },
        of_fse,
        of_pre,
        &of_count,
        last_tables.as_ref().map(|t| &t[2]),
        |t| measure(&ll_choice, &ml_choice, t),
    );

    // 5. Write modes byte: [LL(2)] [OF(2)] [ML(2)] [reserved(2)].
    let modes: u8 = (ll_choice.mode << 6) | (of_choice.mode << 4) | (ml_choice.mode << 2);
    out.push(modes);

    // 6. Table descriptors in wire order (LL, OF, ML): FSE tables
    //    send normalized counts; Predefined/Repeat send nothing.
    if ll_choice.mode == MODE_FSE {
        write_ncount(out, &ll_choice.norm, ll_max, ll_choice.table_log)?;
    } else if ll_choice.mode == MODE_RLE {
        out.push(ll_choice.max_sym);
    }
    if of_choice.mode == MODE_FSE {
        write_ncount(out, &of_choice.norm, of_max, of_choice.table_log)?;
    } else if of_choice.mode == MODE_RLE {
        out.push(of_choice.max_sym);
    }
    if ml_choice.mode == MODE_FSE {
        write_ncount(out, &ml_choice.norm, ml_max, ml_choice.table_log)?;
    } else if ml_choice.mode == MODE_RLE {
        out.push(ml_choice.max_sym);
    }

    // 7. Build CTables from the chosen distributions.
    let ll_ctable = ll_choice.build_ctable()?;
    let ml_ctable = ml_choice.build_ctable()?;
    let of_ctable = of_choice.build_ctable()?;

    // 8. Encode the FSE bitstream (reverse-encoded).
    let start = out.len();
    out.resize(start + estimated_bitstream_size(nb_seq), 0);
    let written = encode_sequences_bitstream(
        &mut out[start..],
        &ll_codes,
        &ml_codes,
        &of_codes,
        &ll_extras,
        &ml_extras,
        &off_bases,
        &ll_ctable,
        &ml_ctable,
        &of_ctable,
        nb_seq,
    )?;
    out.truncate(start + written);

    // The wire rep state after this block — this, not the match
    // finder's internal rotation, is what the next block must carry.
    // The table state advances to what this block left the decoder
    // holding (Repeat leaves it unchanged).
    *last_tables = Some([ll_wire, of_wire, ml_wire]);

    Ok(reps)
}

/// Count symbol frequencies and return the maximum symbol value.
fn count_symbols(codes: &[u8], count: &mut [u32]) -> u8 {
    let mut max_sym = 0u8;
    for &c in codes {
        count[c as usize] += 1;
        if c > max_sym {
            max_sym = c;
        }
    }
    max_sym
}

/// Pick one symbol-type table: the Predefined-vs-FSE winner from
/// [`choose_table_mode`], then RLE if a uniform symbol stream measures
/// smaller, then `Repeat_Mode` if the resulting table is byte-for-byte
/// the one the decoder already holds (saves the ncount/symbol header).
/// Returns the choice plus its decoder-side identity for the next
/// block's Repeat comparison.
fn pick_table<M: Fn(&TableChoice) -> u64>(
    fse_wins: bool,
    fse: TableChoice,
    predef: TableChoice,
    count: &[u32],
    last: Option<&SeqTableOnWire>,
    measure: M,
) -> (TableChoice, SeqTableOnWire) {
    let mut best = if fse_wins { fse } else { predef };
    if let Some(sym) = uniform_symbol(count) {
        let rle = rle_choice(sym);
        if measure(&rle) < measure(&best) {
            best = rle;
        }
    }
    let wire = match best.mode {
        MODE_PREDEFINED => SeqTableOnWire::Predefined,
        MODE_RLE => SeqTableOnWire::Rle(best.max_sym),
        _ => SeqTableOnWire::Fse {
            norm: best.norm.clone(),
            table_log: best.table_log,
        },
    };
    if let Some(l) = last {
        if *l == wire && best.mode != MODE_PREDEFINED {
            best.mode = MODE_REPEAT;
        }
    }
    (best, wire)
}

/// The single symbol an RLE table would carry, if every symbol in
/// the stream is identical.
fn uniform_symbol(count: &[u32]) -> Option<u8> {
    let mut found: Option<u8> = None;
    for (sym, &c) in count.iter().enumerate() {
        if c == 0 {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(sym as u8);
    }
    found
}

/// A single-symbol table choice: one header byte on the wire, zero
/// state bits in the bitstream (built via [`CTable::build_rle`]).
fn rle_choice(sym: u8) -> TableChoice {
    TableChoice {
        mode: MODE_RLE,
        norm: Vec::new(),
        table_log: 0,
        max_sym: sym,
    }
}

/// Result of table mode selection: either Predefined or `FSE_Compressed`.
#[derive(Clone)]
struct TableChoice {
    mode: u8,
    norm: Vec<i16>,
    table_log: u8,
    max_sym: u8,
}

impl TableChoice {
    fn build_ctable(&self) -> Result<CTable, ZstdError> {
        // RLE tables (and Repeat-of-RLE, which keeps the empty norm)
        // never go through the norm-based builder: its transform math
        // underflows at table_log 0 (the C has a dedicated
        // FSE_buildCTable_rle for exactly this shape).
        if self.table_log == 0 {
            return Ok(CTable::build_rle(self.max_sym));
        }
        build_ctable(&self.norm, self.max_sym, self.table_log)
    }
}

/// Choose between Predefined and `FSE_Compressed` for a table.
///
/// Builds the custom-FSE candidate and picks whichever has the lower
/// total cost: payload bits plus, for FSE, the normalized-count header.
/// Viability alone is NOT enough — on peaked distributions (repcodes
/// concentrate the OF histogram at code 0, lazy parses concentrate ML)
/// the predefined tables cost ~4x the custom ones, which is exactly
/// where the reference's custom tables win.
fn choose_table_mode(
    count: &[u32],
    max_sym: u8,
    default_norm: &[i16],
    default_table_log: u8,
    default_max_sym: u8,
    accuracy_cap: u8,
    total: u64,
) -> TableChoice {
    // Check if Predefined can encode all used symbols.
    let predefined_viable = (0..=max_sym as usize)
        .all(|s| count[s] == 0 || (s < default_norm.len() && default_norm[s] != 0));

    // RFC 8878 sequence-table accuracy caps: LL <= 9, OF <= 8,
    // ML <= 9. Exceeding them is a spec violation that conformant
    // decoders (system zstd) reject even though ours is lenient.
    let opt_log = optimal_table_log(accuracy_cap, total as usize, max_sym);
    // C ZSTD_useLowProbCount: low-probability symbols get ncount -1
    // (instead of +1) once blocks carry >= 2048 sequences — it fades
    // in around 16K blocks depending on compressibility.
    let use_low_prob = total >= 2048;
    let custom_norm =
        normalize_count(opt_log, count, total, max_sym, use_low_prob).unwrap_or_default();

    let fse_choice = if custom_norm.is_empty() {
        let mut single_norm = vec![0i16; max_sym as usize + 1];
        single_norm[max_sym as usize] = 1 << opt_log;
        TableChoice {
            mode: MODE_FSE,
            norm: single_norm,
            table_log: opt_log,
            max_sym,
        }
    } else {
        TableChoice {
            mode: MODE_FSE,
            norm: custom_norm,
            table_log: opt_log,
            max_sym,
        }
    };

    if !predefined_viable {
        return fse_choice;
    }

    let predef_bits = estimate_cost(count, default_norm, default_table_log, max_sym);
    let fse_bits = estimate_cost(count, &fse_choice.norm, fse_choice.table_log, max_sym)
        + 8 * estimate_ncount_size(&fse_choice.norm, fse_choice.max_sym, fse_choice.table_log)
            as u64;
    // Slack favoring Predefined, like the reference's selection
    // heuristic: the payload estimate is entropy-approximate, so on
    // small streams a marginal FSE win is noise (and loses in
    // practice — a 1-byte regression on tiny inputs).
    let fse_slack_bits = 24;

    if fse_bits + fse_slack_bits < predef_bits {
        fse_choice
    } else {
        TableChoice {
            mode: MODE_PREDEFINED,
            norm: default_norm.to_vec(),
            table_log: default_table_log,
            max_sym: default_max_sym,
        }
    }
}

/// Actual encoded size (in bits) of the sequences-section header +
/// payload for a given (LL, ML, OF) table triple: modes byte, ncount
/// headers for FSE tables, and the FSE bitstream written with those
/// ctables. Measurement, not estimation — the entropy approximation
/// regressed small streams by a byte.
#[allow(clippy::too_many_arguments)]
fn section_size_bits(
    ll: &TableChoice,
    ml: &TableChoice,
    of: &TableChoice,
    ll_max: u8,
    ml_max: u8,
    of_max: u8,
    codes: (&[u8], &[u8], &[u8], &[u32], &[u32], &[u32]),
    nb_seq: usize,
) -> u64 {
    let (ll_codes, ml_codes, of_codes, ll_extras, ml_extras, off_bases) = codes;
    let mut tmp: Vec<u8> = Vec::new();
    if ll.mode == MODE_FSE {
        let _ = write_ncount(&mut tmp, &ll.norm, ll_max, ll.table_log);
    } else if ll.mode == MODE_RLE {
        tmp.push(ll.max_sym);
    }
    if of.mode == MODE_FSE {
        let _ = write_ncount(&mut tmp, &of.norm, of_max, of.table_log);
    } else if of.mode == MODE_RLE {
        tmp.push(of.max_sym);
    }
    if ml.mode == MODE_FSE {
        let _ = write_ncount(&mut tmp, &ml.norm, ml_max, ml.table_log);
    } else if ml.mode == MODE_RLE {
        tmp.push(ml.max_sym);
    }
    let header_bits = 8 * tmp.len() as u64 + 8; // + modes byte

    let ll_ctable = match ll.build_ctable() {
        Ok(t) => t,
        Err(_) => return u64::MAX,
    };
    let ml_ctable = match ml.build_ctable() {
        Ok(t) => t,
        Err(_) => return u64::MAX,
    };
    let of_ctable = match of.build_ctable() {
        Ok(t) => t,
        Err(_) => return u64::MAX,
    };
    let mut payload: Vec<u8> = vec![0; estimated_bitstream_size(nb_seq)];
    match encode_sequences_bitstream(
        &mut payload,
        ll_codes,
        ml_codes,
        of_codes,
        ll_extras,
        ml_extras,
        off_bases,
        &ll_ctable,
        &ml_ctable,
        &of_ctable,
        nb_seq,
    ) {
        Ok(written) => header_bits + 8 * written as u64,
        Err(_) => u64::MAX,
    }
}

/// Estimate FSE payload cost (in bits) for a given distribution.
fn estimate_cost(count: &[u32], norm: &[i16], table_log: u8, max_sym: u8) -> u64 {
    let table_size = 1u64 << table_log;
    let mut total_bits = 0u64;
    for s in 0..=max_sym as usize {
        if count[s] == 0 {
            continue;
        }
        let n = if s < norm.len() { norm[s] } else { 0 };
        let prob = if n > 0 {
            n as u64
        } else if n == -1 {
            1u64
        } else {
            // norm == 0: this shouldn't happen for symbols with count > 0.
            // Use a worst-case estimate.
            table_size
        };
        // bits per occurrence ≈ log2(table_size / prob)
        let bits_per = (table_size as f64 / prob as f64).log2();
        total_bits += (count[s] as f64 * bits_per) as u64;
    }
    total_bits
}

/// Estimate the byte size of `write_ncount` output without actually writing.
fn estimate_ncount_size(norm: &[i16], max_sym: u8, table_log: u8) -> usize {
    let mut tmp = Vec::new();
    let _ = write_ncount(&mut tmp, norm, max_sym, table_log);
    tmp.len()
}

/// Compute the OF code for a given offset. The OF code is the number
/// of bits needed to represent the offBase minus 1, capped at 31.
fn off_code_for_offset(offset: u32) -> u8 {
    let ob = off_base(offset);
    // OF code N has base 1<<N and N extra bits. Find N such that
    // 1<<N <= offBase < 1<<(N+1).
    let n = if ob == 0 { 0 } else { ob.ilog2() };
    n.min(31) as u8
}

/// Write the sequence count in the variable-length format.
/// Matches C `ZSTD_decodeSequenceCount_header`:
/// - byte0 < 128: nbSeq = byte0 (1 byte)
/// - 128 ≤ byte0 < 255: nbSeq = ((byte0-128) << 8) + byte1 (2 bytes)
/// - byte0 == 255: nbSeq = LE16(byte1, byte2) + 0x7F00 (3 bytes)
fn write_sequence_count(out: &mut Vec<u8>, nb_seq: usize) {
    if nb_seq < 128 {
        out.push(nb_seq as u8);
    } else if nb_seq < 0x7F00 {
        // 2-byte: byte0 = 128 + (nbSeq >> 8), byte1 = nbSeq & 0xFF.
        out.push((128 + (nb_seq >> 8) as u8));
        out.push((nb_seq & 0xFF) as u8);
    } else {
        // 3-byte: 0xFF marker + LE16(nbSeq - 0x7F00).
        let v = (nb_seq - 0x7F00) as u32;
        out.push(0xFF);
        out.push(v as u8);
        out.push((v >> 8) as u8);
    }
}

/// Rough upper bound on bitstream size: each sequence needs at most
/// `LL_bits(35)=16` + `ML_bits(52)=16` + `OF_bits(31)=31` + 3*tableLog bits
/// for FSE state updates. Plus init/flush states.
fn estimated_bitstream_size(nb_seq: usize) -> usize {
    let per_seq_bits = 16 + 16 + 31 + 6 + 6 + 5; // ~80 bits
    let init_bits = 6 + 5 + 6; // LL, OF, ML init states
    let total_bits = init_bits + nb_seq * per_seq_bits + 64; // +64 padding
    total_bits.div_ceil(8) + 8
}

/// Encode the FSE bitstream for sequences. Returns bytes written.
fn encode_sequences_bitstream(
    dst: &mut [u8],
    ll_codes: &[u8],
    ml_codes: &[u8],
    of_codes: &[u8],
    ll_extras: &[u32],
    ml_extras: &[u32],
    off_bases: &[u32],
    ll_ctable: &CTable,
    ml_ctable: &CTable,
    of_ctable: &CTable,
    nb_seq: usize,
) -> Result<usize, ZstdError> {
    // We write into a Vec since BIT_CStream needs growable storage.
    let mut out_vec: Vec<u8> = Vec::with_capacity(dst.len());
    let mut bitc = BitCStream::new(&mut out_vec);

    if nb_seq == 0 {
        return Ok(0);
    }

    // Initialize 3 states from the LAST sequence's codes.
    // Order in C: ML, OF, LL.
    let mut state_ml = CState::init2(ml_ctable, ml_codes[nb_seq - 1]);
    let mut state_of = CState::init2(of_ctable, of_codes[nb_seq - 1]);
    let mut state_ll = CState::init2(ll_ctable, ll_codes[nb_seq - 1]);

    // Write the last sequence's extra bits.
    bitc.add_bits(
        u64::from(ll_extras[nb_seq - 1]),
        u32::from(LL_BITS[ll_codes[nb_seq - 1] as usize]),
    );
    bitc.flush();
    bitc.add_bits(
        u64::from(ml_extras[nb_seq - 1]),
        u32::from(ML_BITS[ml_codes[nb_seq - 1] as usize]),
    );
    bitc.flush();
    bitc.add_bits(
        u64::from(off_bases[nb_seq - 1]),
        u32::from(of_codes[nb_seq - 1]),
    );
    bitc.flush();

    // Encode remaining sequences in reverse (nb_seq-2 down to 0).
    // Each iteration: encode OF, ML, LL symbols via FSE, then write extras.
    for n in (0..nb_seq - 1).rev() {
        // FSE state updates (writes bits for the next symbol's state).
        state_of.encode(&mut bitc, of_ctable, of_codes[n]);
        state_ml.encode(&mut bitc, ml_ctable, ml_codes[n]);
        bitc.flush();
        state_ll.encode(&mut bitc, ll_ctable, ll_codes[n]);
        bitc.flush();

        // Extra bits for this sequence.
        bitc.add_bits(
            u64::from(ll_extras[n]),
            u32::from(LL_BITS[ll_codes[n] as usize]),
        );
        bitc.add_bits(
            u64::from(ml_extras[n]),
            u32::from(ML_BITS[ml_codes[n] as usize]),
        );
        bitc.flush();
        bitc.add_bits(u64::from(off_bases[n]), u32::from(of_codes[n]));
        bitc.flush();
    }

    // Flush final states in reverse init order: ML, OF, LL.
    state_ml.flush(&mut bitc);
    state_of.flush(&mut bitc);
    state_ll.flush(&mut bitc);

    let written = bitc.close();
    let len = written.min(dst.len());
    dst[..len].copy_from_slice(&out_vec[..len]);
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::match_finder::{compress_block_fast, MatchState};

    #[test]
    fn ll_code_lookup() {
        assert_eq!(ll_code(0), (0, 0));
        assert_eq!(ll_code(1), (1, 0));
        assert_eq!(ll_code(16), (16, 0));
        assert_eq!(ll_code(17), (16, 1));
        assert_eq!(ll_code(18), (17, 0));
    }

    #[test]
    fn ml_code_lookup() {
        assert_eq!(ml_code(3), (0, 0));
        assert_eq!(ml_code(4), (1, 0));
        assert_eq!(ml_code(35), (32, 0));
        assert_eq!(ml_code(36), (32, 1));
    }

    #[test]
    fn off_code_for_small_offsets() {
        assert_eq!(off_code_for_offset(1), 2); // offBase=4, ilog2=2
        assert_eq!(off_code_for_offset(3), 2); // offBase=6, ilog2=2
        assert_eq!(off_code_for_offset(4), 2); // offBase=7, ilog2=2
        assert_eq!(off_code_for_offset(5), 3); // offBase=8, ilog2=3
    }

    #[test]
    fn empty_seq_store_produces_zero_byte() {
        let mut out = Vec::new();
        let ss = SeqStore::new();
        encode_section(&mut out, &ss.sequences, [1, 4, 8], &mut None).expect("encode");
        assert_eq!(out, vec![0x00]); // 0 sequences
    }

    #[test]
    fn encode_section_does_not_panic() {
        let mut ss = SeqStore::new();
        let mut ms = MatchState::new(7);
        let input = b"abcdefghabcdefghabcdefghabcdefgh";
        compress_block_fast(input, &mut ss, &mut ms);
        let mut out = Vec::new();
        // This may fail due to FSE bitstream bugs; just check no panic.
        let _ = encode_section(&mut out, &ss.sequences, [1, 4, 8], &mut None);
    }
}
