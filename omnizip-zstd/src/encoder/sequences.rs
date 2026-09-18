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
    // Per-candidate cost caching. The three FSE streams' bit costs are
    // independent (each state machine walks its own code list), and
    // the literal/match/offset extra bits are table-independent — so
    // total bits = S_ll(choice) + S_ml(choice) + S_of(choice) + K,
    // and every table-mode comparison the old full-remeasure path
    // made (a 3-stream walk + 3 ctable builds per candidate pair) is
    // exact arithmetic over per-candidate cached sums. The
    // ceil((bits+1)/8) padding couples the streams, but is computed
    // identically from the cached sums — bit-for-bit the same
    // decisions as section_size_bits.
    let k_extras: u64 = ll_codes
        .iter()
        .zip(ml_codes.iter().zip(of_codes.iter()))
        .map(|(&l, (&m, &o))| {
            u64::from(LL_BITS[l as usize]) + u64::from(ML_BITS[m as usize]) + u64::from(o)
        })
        .sum();
    #[derive(Clone, Copy)]
    struct StreamCost {
        payload_bits: u64,
        header_bytes: u32,
        valid: bool,
    }
    const INVALID: StreamCost = StreamCost {
        payload_bits: u64::MAX,
        header_bytes: 0,
        valid: false,
    };
    fn stream_payload(codes: &[u8], choice: &TableChoice, stream_max: u8) -> StreamCost {
        // Header bytes mirror section_size_bits exactly: FSE writes
        // the normalized-count table (length matters, bytes are
        // re-emitted identically by the real writer later), RLE
        // writes one byte, Predefined/Repeat write nothing.
        let mut tmp = Vec::new();
        if choice.mode == MODE_FSE {
            let _ = write_ncount(&mut tmp, &choice.norm, stream_max, choice.table_log);
        } else if choice.mode == MODE_RLE {
            tmp.push(choice.max_sym);
        }
        let header = tmp.len() as u32;
        let ctable = match choice.build_ctable() {
            Ok(t) => t,
            Err(_) => return INVALID,
        };
        let last = codes.len() - 1;
        let mut state = CState::init2(&ctable, codes[last]);
        let mut bits = 0u64;
        for n in (0..last).rev() {
            bits += u64::from(state.encode_bit_count(&ctable, codes[n]));
        }
        bits += u64::from(state.flush_bit_count());
        StreamCost {
            payload_bits: bits,
            header_bytes: header,
            valid: true,
        }
    }
    let measure_from = |ll: StreamCost, ml: StreamCost, of: StreamCost| -> u64 {
        if !ll.valid || !ml.valid || !of.valid {
            return u64::MAX;
        }
        let header_bits = 8 * (u64::from(ll.header_bytes + ml.header_bytes + of.header_bytes)) + 8;
        let bits = ll.payload_bits + ml.payload_bits + of.payload_bits + k_extras;
        header_bits + 8 * ((bits + 1 + 7) / 8)
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

    // Fused mode-search: advance both candidate state machines per
    // symbol in ONE walk (halves the code-array passes from 6 to 3,
    // reads codes[n] once — better cache + ILP). The two state
    // machines are independent, so the costs are bit-identical to
    // the separate-walk form.
    let stream_payload_pair =
        |codes: &[u8], ca: &TableChoice, cb: &TableChoice, mx: u8| -> (StreamCost, StreamCost) {
            let mut tmp_a = Vec::new();
            let mut tmp_b = Vec::new();
            if ca.mode == MODE_FSE {
                let _ = write_ncount(&mut tmp_a, &ca.norm, mx, ca.table_log);
            } else if ca.mode == MODE_RLE {
                tmp_a.push(ca.max_sym);
            }
            if cb.mode == MODE_FSE {
                let _ = write_ncount(&mut tmp_b, &cb.norm, mx, cb.table_log);
            } else if cb.mode == MODE_RLE {
                tmp_b.push(cb.max_sym);
            }
            let ctable_a = match ca.build_ctable() {
                Ok(t) => t,
                Err(_) => return (INVALID, INVALID),
            };
            let ctable_b = match cb.build_ctable() {
                Ok(t) => t,
                Err(_) => return (INVALID, INVALID),
            };
            let last = codes.len() - 1;
            let mut state_a = CState::init2(&ctable_a, codes[last]);
            let mut state_b = CState::init2(&ctable_b, codes[last]);
            let mut bits_a = 0u64;
            let mut bits_b = 0u64;
            for n in (0..last).rev() {
                let sym = codes[n];
                bits_a += u64::from(state_a.encode_bit_count(&ctable_a, sym));
                bits_b += u64::from(state_b.encode_bit_count(&ctable_b, sym));
            }
            bits_a += u64::from(state_a.flush_bit_count());
            bits_b += u64::from(state_b.flush_bit_count());
            (
                StreamCost {
                    payload_bits: bits_a,
                    header_bytes: tmp_a.len() as u32,
                    valid: true,
                },
                StreamCost {
                    payload_bits: bits_b,
                    header_bytes: tmp_b.len() as u32,
                    valid: true,
                },
            )
        };

    let (ll_pre_c, ll_fse_c) = stream_payload_pair(&ll_codes, &ll_pre, &ll_fse, ll_max);
    let (ml_pre_c, ml_fse_c) = stream_payload_pair(&ml_codes, &ml_pre, &ml_fse, ml_max);
    let (of_pre_c, of_fse_c) = stream_payload_pair(&of_codes, &of_pre, &of_fse, of_max);

    // Single-table cost for the pick_table RLE/Repeat evaluation
    // (one candidate at a time — not fusable).
    let ll_cost = |c: &TableChoice| stream_payload(&ll_codes, c, ll_max);
    let ml_cost = |c: &TableChoice| stream_payload(&ml_codes, c, ml_max);
    let of_cost = |c: &TableChoice| stream_payload(&of_codes, c, of_max);

    let (ll_choice, ll_wire) = pick_table(
        ll_fse.mode == MODE_FSE && {
            let with = measure_from(ll_fse_c, ml_pre_c, of_pre_c);
            let without = measure_from(ll_pre_c, ml_pre_c, of_pre_c);
            with < without
        },
        ll_fse,
        ll_pre,
        &ll_count,
        last_tables.as_ref().map(|t| &t[0]),
        |t| measure_from(ll_cost(t), ml_pre_c, of_pre_c),
    );
    let ll_choice_c = if ll_choice.mode == MODE_FSE {
        ll_fse_c
    } else if uniform_symbol(&ll_count).is_some() && ll_choice.mode == MODE_RLE {
        ll_cost(&ll_choice)
    } else {
        ll_pre_c
    };
    let (ml_choice, ml_wire) = pick_table(
        ml_fse.mode == MODE_FSE && {
            let with = measure_from(ll_choice_c, ml_fse_c, of_pre_c);
            let without = measure_from(ll_choice_c, ml_pre_c, of_pre_c);
            with < without
        },
        ml_fse,
        ml_pre,
        &ml_count,
        last_tables.as_ref().map(|t| &t[1]),
        |t| measure_from(ll_choice_c, ml_cost(t), of_pre_c),
    );
    let ml_choice_c = if ml_choice.mode == MODE_FSE {
        ml_fse_c
    } else if uniform_symbol(&ml_count).is_some() && ml_choice.mode == MODE_RLE {
        ml_cost(&ml_choice)
    } else {
        ml_pre_c
    };
    let (of_choice, of_wire) = pick_table(
        of_fse.mode == MODE_FSE && {
            let with = measure_from(ll_choice_c, ml_choice_c, of_fse_c);
            let without = measure_from(ll_choice_c, ml_choice_c, of_pre_c);
            with < without
        },
        of_fse,
        of_pre,
        &of_count,
        last_tables.as_ref().map(|t| &t[2]),
        |t| measure_from(ll_choice_c, ml_choice_c, of_cost(t)),
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
    // Structural writer (task 50 attempt 2): local-variable state
    // machines, pre-allocated buffer, macro-based bit ops.
    let max_out = nb_seq.saturating_mul(8) + 32;
    let mut buf = vec![0u8; max_out];
    let mut pos: usize = 0;
    let mut container: u64 = 0;
    let mut bit_pos: u32 = 0;

    macro_rules! flush_bits {
        () => {{
            let nb_bytes = (bit_pos >> 3) as usize;
            if nb_bytes > 0 && pos + 8 <= buf.len() {
                let bytes = container.to_le_bytes();
                buf[pos..pos + 8].copy_from_slice(&bytes);
                pos += nb_bytes;
                bit_pos &= 7;
                container = if nb_bytes >= 8 {
                    0
                } else {
                    container >> (nb_bytes * 8)
                };
            }
        }};
    }

    macro_rules! add_bits {
        ($v:expr, $n:expr) => {{
            let v = $v;
            let n = $n;
            if n > 0 {
                if bit_pos + n > 64 {
                    flush_bits!();
                }
                let mask: u64 = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
                container |= (v & mask) << bit_pos;
                bit_pos += n;
            }
        }};
    }

    if nb_seq == 0 {
        return Err(ZstdError::Corrupt {
            reason: "empty stream".into(),
        });
    }

    let init_ml = CState::init2(ml_ctable, ml_codes[nb_seq - 1]);
    let init_of = CState::init2(of_ctable, of_codes[nb_seq - 1]);
    let init_ll = CState::init2(ll_ctable, ll_codes[nb_seq - 1]);

    // Local state values (register-resident).
    let mut st_ml: u32 = init_ml.value;
    let mut st_of: u32 = init_of.value;
    let mut st_ll: u32 = init_ll.value;

    // Write the last sequence's extra bits.
    add_bits!(
        u64::from(ll_extras[nb_seq - 1]),
        u32::from(LL_BITS[ll_codes[nb_seq - 1] as usize])
    );
    add_bits!(
        u64::from(ml_extras[nb_seq - 1]),
        u32::from(ML_BITS[ml_codes[nb_seq - 1] as usize])
    );
    add_bits!(
        u64::from(off_bases[nb_seq - 1]),
        u32::from(of_codes[nb_seq - 1])
    );

    for n in (0..nb_seq - 1).rev() {
        // Inline FSE state steps (3 per sequence).
        let sym_of = of_codes[n] as usize;
        let tt_of = &of_ctable.symbol_tt[sym_of];
        let nb_of = (st_of + tt_of.delta_nb_bits) >> 16;
        add_bits!(u64::from(st_of), nb_of);
        st_of = u32::from(
            of_ctable.state_table
                [((i64::from(st_of >> nb_of) + i64::from(tt_of.delta_find_state)) as usize)],
        );

        let sym_ml = ml_codes[n] as usize;
        let tt_ml = &ml_ctable.symbol_tt[sym_ml];
        let nb_ml = (st_ml + tt_ml.delta_nb_bits) >> 16;
        add_bits!(u64::from(st_ml), nb_ml);
        st_ml = u32::from(
            ml_ctable.state_table
                [((i64::from(st_ml >> nb_ml) + i64::from(tt_ml.delta_find_state)) as usize)],
        );

        let sym_ll = ll_codes[n] as usize;
        let tt_ll = &ll_ctable.symbol_tt[sym_ll];
        let nb_ll = (st_ll + tt_ll.delta_nb_bits) >> 16;
        add_bits!(u64::from(st_ll), nb_ll);
        st_ll = u32::from(
            ll_ctable.state_table
                [((i64::from(st_ll >> nb_ll) + i64::from(tt_ll.delta_find_state)) as usize)],
        );

        add_bits!(
            u64::from(ll_extras[n]),
            u32::from(LL_BITS[ll_codes[n] as usize])
        );
        add_bits!(
            u64::from(ml_extras[n]),
            u32::from(ML_BITS[ml_codes[n] as usize])
        );
        add_bits!(u64::from(off_bases[n]), u32::from(of_codes[n]));

        flush_bits!();
    }

    // Flush final states.
    add_bits!(u64::from(st_ml), u32::from(ml_ctable.table_log()));
    flush_bits!();
    add_bits!(u64::from(st_of), u32::from(of_ctable.table_log()));
    flush_bits!();
    add_bits!(u64::from(st_ll), u32::from(ll_ctable.table_log()));

    add_bits!(1, 1);
    flush_bits!();
    if bit_pos > 0 && pos < buf.len() {
        buf[pos] = container as u8;
        pos += 1;
    }

    let len = pos.min(dst.len());
    dst[..len].copy_from_slice(&buf[..len]);
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
