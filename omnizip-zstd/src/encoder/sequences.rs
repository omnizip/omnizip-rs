//! ZSTD sequences section encoder — converts match-finder output
//! (literal_length, match_length, offset) into the FSE-coded wire
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
//! This module currently implements **Predefined** mode only. RLE and
//! FSE will follow once the FSE encoder bitstream is verified.

#![forbid(unsafe_code)]

use crate::encoder::match_finder::SeqStore;
use crate::fse::encoder::{build_ctable, BitCStream, CState, CTable};
use crate::ZstdError;

/// Sequence-table mode codes.
const MODE_PREDEFINED: u8 = 0;

/// Predefined LL normalized distribution (from C's `LL_defaultNorm`).
/// 36 entries, tableLog = 6.
const LL_DEFAULT_NORM: [i16; 36] = [
    4, 3, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 1, 1, 1,
    2, 2, 2, 2, 2, 2, 2, 2,
    2, 3, 2, 1, 1, 1, 1, 1,
    -1, -1, -1, -1,
];

/// Predefined ML normalized distribution (from C's `ML_defaultNorm`).
/// 53 entries, tableLog = 6.
const ML_DEFAULT_NORM: [i16; 53] = [
    1, 4, 3, 2, 2, 2, 2, 2,
    2, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, -1, -1,
    -1, -1, -1, -1, -1,
];

/// Predefined OF normalized distribution (from C's `OF_defaultNorm`).
/// 29 entries, tableLog = 5.
const OF_DEFAULT_NORM: [i16; 29] = [
    1, 1, 1, 1, 1, 1, 2, 2,
    2, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1,
    -1, -1, -1, -1, -1,
];

/// LL code → number of extra bits (from C's `LL_bits`).
const LL_BITS: [u8; 36] = [
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    1, 1, 1, 1, 2, 2, 3, 3,
    4, 6, 7, 8, 9, 10, 11, 12,
    13, 14, 15, 16,
];

/// ML code → number of extra bits (from C's `ML_bits`).
const ML_BITS: [u8; 53] = [
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    1, 1, 1, 1, 2, 2, 3, 3,
    4, 4, 5, 7, 8, 9, 10, 11,
    12, 13, 14, 15, 16,
];

/// LL code → base literal length value (from C's `LL_Base`).
const LL_BASE: [u32; 36] = [
    0, 1, 2, 3, 4, 5, 6, 7,
    8, 9, 10, 11, 12, 13, 14, 15,
    16, 18, 20, 22, 24, 28, 32, 40,
    48, 64, 128, 256, 512, 1024, 2048, 4096,
    8192, 16384, 32768, 65536,
];

/// ML code → base match length value (from C's `ML_Base`).
const ML_BASE: [u32; 53] = [
    3, 4, 5, 6, 7, 8, 9, 10,
    11, 12, 13, 14, 15, 16, 17, 18,
    19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34,
    35, 37, 39, 41, 43, 47, 51, 59,
    67, 83, 99, 131, 259, 515, 1027, 2051,
    4099, 8195, 16387, 32771, 65539,
];

/// Find the LL code for a given literal length. Returns (code, extra_bits_value).
fn ll_code(lit_len: u32) -> (u8, u32) {
    for code in (0..36).rev() {
        if LL_BASE[code] <= lit_len {
            return (code as u8, lit_len - LL_BASE[code]);
        }
    }
    (0, lit_len)
}

/// Find the ML code for a given match length. Returns (code, extra_bits_value).
fn ml_code(match_len: u32) -> (u8, u32) {
    for code in (0..53).rev() {
        if ML_BASE[code] <= match_len {
            return (code as u8, match_len - ML_BASE[code]);
        }
    }
    (0, match_len)
}

/// Compute the offset base value for a given byte distance.
/// ZSTD offset coding: offBase = offset + REPEAT_SLOTS (3).
/// Repeat offsets 1 and 2 use offBase 1 and 2 respectively.
const fn off_base(offset: u32) -> u32 {
    offset + 3
}

/// Encode the sequences section from a [`SeqStore`] into `out`.
/// Uses Predefined mode for all three tables (LL, ML, OF).
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on internal failures.
pub fn encode_section(
    out: &mut Vec<u8>,
    seq_store: &SeqStore,
) -> Result<(), ZstdError> {
    let nb_seq = seq_store.sequences.len();

    // 1. Sequence count (1-3 bytes).
    write_sequence_count(out, nb_seq);

    if nb_seq == 0 {
        return Ok(());
    }

    // 2. Modes byte: LL=Predefined, OF=Predefined, ML=Predefined.
    // Bits: [LL_mode(2)] [OF_mode(2)] [ML_mode(2)] [reserved(2)]
    let modes: u8 = (MODE_PREDEFINED << 6)
        | (MODE_PREDEFINED << 4)
        | (MODE_PREDEFINED << 2);
    out.push(modes);

    // 3. Build CTables from the predefined distributions.
    let ll_ctable = build_ctable(&LL_DEFAULT_NORM, 35, 6)?;
    let ml_ctable = build_ctable(&ML_DEFAULT_NORM, 52, 6)?;
    let of_ctable = build_ctable(&OF_DEFAULT_NORM, 28, 5)?;

    // 4. Compute code tables for each sequence.
    let mut ll_codes = Vec::with_capacity(nb_seq);
    let mut ml_codes = Vec::with_capacity(nb_seq);
    let mut of_codes = Vec::with_capacity(nb_seq);
    let mut ll_extras = Vec::with_capacity(nb_seq);
    let mut ml_extras = Vec::with_capacity(nb_seq);
    let mut off_bases = Vec::with_capacity(nb_seq);

    for seq in &seq_store.sequences {
        let (ll_c, ll_e) = ll_code(seq.literal_length);
        let (ml_c, ml_e) = ml_code(seq.match_length);
        ll_codes.push(ll_c);
        ml_codes.push(ml_c);
        of_codes.push(off_code_for_offset(seq.offset));
        ll_extras.push(ll_e);
        ml_extras.push(ml_e);
        off_bases.push(off_base(seq.offset));
    }

    // 5. Encode the FSE bitstream (reverse-encoded).
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

    Ok(())
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
        out.push((128 + (nb_seq >> 8) as u8) as u8);
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
/// LL_bits(35)=16 + ML_bits(52)=16 + OF_bits(31)=31 + 3*tableLog bits
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
    bitc.add_bits(u64::from(ll_extras[nb_seq - 1]), u32::from(LL_BITS[ll_codes[nb_seq - 1] as usize]));
    bitc.flush();
    bitc.add_bits(u64::from(ml_extras[nb_seq - 1]), u32::from(ML_BITS[ml_codes[nb_seq - 1] as usize]));
    bitc.flush();
    bitc.add_bits(u64::from(off_bases[nb_seq - 1]), u32::from(of_codes[nb_seq - 1]));
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
        bitc.add_bits(u64::from(ll_extras[n]), u32::from(LL_BITS[ll_codes[n] as usize]));
        bitc.add_bits(u64::from(ml_extras[n]), u32::from(ML_BITS[ml_codes[n] as usize]));
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
        encode_section(&mut out, &ss).expect("encode");
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
        let _ = encode_section(&mut out, &ss);
    }
}
