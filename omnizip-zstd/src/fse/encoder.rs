//! FSE (Finite State Entropy) encoder — port of
//! `~/src/external/zstd/lib/compress/fse_compress.c`.
//!
//! Produces the wire format that `fse::from_stream::read_fse_table` and
//! `fse::interleaved::decode_stream` consume on the decode side.
//!
//! ## Pipeline
//!
//! 1. [`normalize_count`] — scale raw histogram to `1 << tableLog`.
//! 2. [`build_ctable`] — build the encoding table (`CTable`).
//! 3. [`write_ncount`] — emit the probability-table header.
//! 4. [`compress_using_ctable`] — emit the interleaved bitstream.
//!
//! ## Bitstream layout
//!
//! The encoder writes bytes forward but accumulates bits at the LOW end
//! of a u64 container (matching C's `BIT_CStream`). The decoder reads
//! backward from the last byte. The encoder processes symbols in
//! REVERSE input order so the decoder reads them in forward order.

#![forbid(unsafe_code)]

use crate::constants::{FSE_MAX_ACCURACY_LOG, FSE_MIN_ACCURACY_LOG};
use crate::ZstdError;

/// Default tableLog when none is specified.
const FSE_DEFAULT_TABLELOG: u8 = 6;

/// Per-symbol transform used during encoding. Matches C's
/// `FSE_symbolCompressionTransform`.
#[derive(Clone, Copy, Debug, Default)]
struct SymbolCompressionTransform {
    /// Offset into `state_table` for this symbol's state range.
    delta_find_state: i32,
    /// `(maxBitsOut << 16) - minStatePlus`. During encoding,
    /// `nbBitsOut = (state + delta_nb_bits) >> 16`.
    delta_nb_bits: u32,
}

/// FSE encoding table. Built once per probability distribution and
/// reused for every symbol in the stream.
#[derive(Clone, Debug)]
pub struct CTable {
    table_log: u8,
    max_symbol_value: u8,
    /// State transition table: `state_table[symbol_offset + intra]`
    /// gives the next state value. Indexed via
    /// `(state >> nbBitsOut) + delta_find_state`.
    state_table: Vec<u16>,
    /// Per-symbol transform parameters.
    symbol_tt: Vec<SymbolCompressionTransform>,
}

impl CTable {
    /// Build an RLE `CTable` for a single-symbol stream. The encoded
    /// bitstream is empty; the decoder reproduces the symbol from the
    /// table header. Matches C's `FSE_buildCTable_rle`.
    #[must_use]
    pub fn build_rle(symbol: u8) -> Self {
        Self {
            table_log: 0,
            max_symbol_value: symbol,
            state_table: vec![0, 0],
            symbol_tt: vec![SymbolCompressionTransform::default(); usize::from(symbol) + 1],
        }
    }

    #[must_use]
    pub const fn table_log(&self) -> u8 {
        self.table_log
    }

    #[must_use]
    pub const fn max_symbol_value(&self) -> u8 {
        self.max_symbol_value
    }
}

/// Choose the optimal `tableLog` for the given input size and alphabet.
/// Ported from C's `FSE_optimalTableLog` (which calls
/// `FSE_optimalTableLog_internal` with `minus = 2`).
#[must_use]
pub fn optimal_table_log(max_table_log: u8, src_size: usize, max_symbol_value: u8) -> u8 {
    optimal_table_log_internal(max_table_log, src_size, max_symbol_value, 2)
}

/// Internal variant with configurable `minus` offset.
fn optimal_table_log_internal(
    max_table_log: u8,
    src_size: usize,
    max_symbol_value: u8,
    minus: u32,
) -> u8 {
    if src_size <= 1 {
        return FSE_MIN_ACCURACY_LOG;
    }
    let max_bits_src = (src_size as u32 - 1).ilog2().saturating_sub(minus);
    let min_bits_src = (src_size as u32).ilog2() + 1;
    let min_bits_sym = u32::from(max_symbol_value).ilog2() + 2;
    let min_bits = min_bits_src.min(min_bits_sym);
    let mut table_log = max_table_log;
    if table_log == 0 {
        table_log = FSE_DEFAULT_TABLELOG;
    }
    if max_bits_src < u32::from(table_log) {
        table_log = max_bits_src as u8;
    }
    if min_bits > u32::from(table_log) {
        table_log = min_bits as u8;
    }
    table_log
        .max(FSE_MIN_ACCURACY_LOG)
        .min(FSE_MAX_ACCURACY_LOG)
}

/// Normalize a raw histogram to sum to `1 << tableLog`. Ported from
/// C's `FSE_normalizeCount`.
///
/// `count[s]` is the raw frequency of symbol `s`. `total` is
/// `sum(count)`. Returns the normalized distribution as `i16` values
/// (positive = cell count, -1 = low-probability sentinel, 0 = absent).
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] if the distribution can't be
/// normalized (e.g. RLE stream where all symbols are the same).
pub fn normalize_count(
    table_log: u8,
    count: &[u32],
    total: u64,
    max_symbol_value: u8,
    use_low_prob_count: bool,
) -> Result<Vec<i16>, ZstdError> {
    let table_log = if table_log == 0 {
        FSE_DEFAULT_TABLELOG
    } else {
        table_log
    };
    if !(FSE_MIN_ACCURACY_LOG..=FSE_MAX_ACCURACY_LOG).contains(&table_log) {
        return Err(ZstdError::Corrupt {
            reason: format!("FSE normalize: tableLog {table_log} out of range"),
        });
    }

    let mut norm = vec![0i16; usize::from(max_symbol_value) + 1];

    // RLE check.
    for &c in &count[..=max_symbol_value as usize] {
        if u64::from(c) == total {
            return Ok(vec![]);
        }
    }

    const RTB_TABLE: [u32; 8] = [0, 473195, 504333, 520860, 550000, 700000, 750000, 830000];
    let low_prob_count: i16 = if use_low_prob_count { -1 } else { 1 };
    let scale = 62u32 - u32::from(table_log);
    let step = (1u64 << 62) / total;
    let v_step = 1u64 << (scale - 20);
    let mut still_to_distribute = 1i32 << table_log;
    let low_threshold = total >> table_log;
    let mut largest = 0usize;
    let mut largest_p: i16 = 0;

    for s in 0..=usize::from(max_symbol_value) {
        if count[s] == 0 {
            norm[s] = 0;
            continue;
        }
        let c64 = u64::from(count[s]);
        if c64 <= low_threshold {
            norm[s] = low_prob_count;
            still_to_distribute -= 1;
        } else {
            let mut proba = ((c64 * step) >> scale) as i16;
            if proba < 8 {
                let rest_to_beat = v_step * u64::from(RTB_TABLE[proba as usize]);
                let diff = c64 * step - ((proba as u64) << scale);
                if diff > rest_to_beat {
                    proba += 1;
                }
            }
            if proba > largest_p {
                largest_p = proba;
                largest = s;
            }
            norm[s] = proba;
            still_to_distribute -= i32::from(proba);
        }
    }

    if -still_to_distribute >= i32::from(norm[largest]) / 2 {
        // Corner case: fall back to secondary normalization.
        normalize_m2(
            &mut norm,
            table_log,
            count,
            total,
            max_symbol_value,
            low_prob_count,
        )?;
    } else {
        norm[largest] += still_to_distribute as i16;
    }

    Ok(norm)
}

/// Secondary normalization method (C's `FSE_normalizeM2`). Used when
/// the primary method's largest-symbol correction would be too large.
fn normalize_m2(
    norm: &mut [i16],
    table_log: u8,
    count: &[u32],
    total: u64,
    max_symbol_value: u8,
    low_prob_count: i16,
) -> Result<(), ZstdError> {
    const NOT_YET_ASSIGNED: i16 = -2;
    let table_size = 1u32 << table_log;
    let low_threshold = total >> table_log;
    let mut low_one = (total * 3) >> (table_log + 1);
    let mut distributed = 0u32;
    let mut remaining = total;

    for s in 0..=usize::from(max_symbol_value) {
        if count[s] == 0 {
            norm[s] = 0;
            continue;
        }
        let c = u64::from(count[s]);
        if c <= low_threshold {
            norm[s] = low_prob_count;
            distributed += 1;
            remaining -= c;
            continue;
        }
        if c <= low_one {
            norm[s] = 1;
            distributed += 1;
            remaining -= c;
            continue;
        }
        norm[s] = NOT_YET_ASSIGNED;
    }

    let mut to_distribute = table_size - distributed;
    if to_distribute == 0 {
        return Ok(());
    }

    if remaining / u64::from(to_distribute) > low_one {
        low_one = (remaining * 3) / (u64::from(to_distribute) * 2);
        for s in 0..=usize::from(max_symbol_value) {
            if norm[s] == NOT_YET_ASSIGNED && u64::from(count[s]) <= low_one {
                norm[s] = 1;
                distributed += 1;
                remaining -= u64::from(count[s]);
            }
        }
        to_distribute = table_size - distributed;
    }

    if distributed == u32::from(max_symbol_value) + 1 {
        // All values are poor; give remaining to the max.
        let mut max_v = 0;
        let mut max_c = 0u32;
        for s in 0..=usize::from(max_symbol_value) {
            if count[s] > max_c {
                max_v = s;
                max_c = count[s];
            }
        }
        norm[max_v] += to_distribute as i16;
        return Ok(());
    }

    if remaining == 0 {
        let mut idx = 0;
        while to_distribute > 0 {
            if norm[idx] > 0 {
                norm[idx] += 1;
                to_distribute -= 1;
            }
            idx = (idx + 1) % (usize::from(max_symbol_value) + 1);
        }
        return Ok(());
    }

    let v_step_log = 62u32 - u32::from(table_log);
    let mid = (1u64 << (v_step_log - 1)) - 1;
    let r_step = (((1u64 << v_step_log) * u64::from(to_distribute)) + mid) / remaining;
    let mut tmp_total = mid;
    for s in 0..=usize::from(max_symbol_value) {
        if norm[s] == NOT_YET_ASSIGNED {
            let end = tmp_total + u64::from(count[s]) * r_step;
            let s_start = (tmp_total >> v_step_log) as u32;
            let s_end = (end >> v_step_log) as u32;
            let weight = s_end.saturating_sub(s_start);
            if weight < 1 {
                return Err(ZstdError::Corrupt {
                    reason: "FSE normalize_m2: weight < 1".into(),
                });
            }
            norm[s] = weight as i16;
            tmp_total = end;
        }
    }
    Ok(())
}

/// Build a [`CTable`] from a normalized probability distribution.
/// Ported from C's `FSE_buildCTable_wksp`.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on invalid input.
pub fn build_ctable(
    norm: &[i16],
    max_symbol_value: u8,
    table_log: u8,
) -> Result<CTable, ZstdError> {
    let table_size = 1usize << table_log;
    let step = (table_size >> 1) + (table_size >> 3) + 3; // FSE_TABLESTEP
    let mask = table_size - 1;
    let max_sv1 = usize::from(max_symbol_value) + 1;

    let mut table_symbol = vec![0xFFFFu16; table_size];
    let mut high_threshold = table_size - 1;

    // Phase 1: Lay down low-prob symbols at the top, compute cumulative.
    let mut cumul = vec![0u16; max_sv1 + 1];
    cumul[0] = 0;
    for u in 1..=max_sv1 {
        if norm[u - 1] == -1 {
            cumul[u] = cumul[u - 1] + 1;
            table_symbol[high_threshold] = (u - 1) as u16;
            high_threshold -= 1;
        } else {
            cumul[u] = cumul[u - 1] + norm[u - 1] as u16;
        }
    }
    cumul[max_sv1] = (table_size + 1) as u16;

    // Phase 2: Spread positive-count symbols (same as decode side).
    let mut position = 0usize;
    for symbol in 0..max_sv1 {
        let freq = norm[symbol];
        if freq <= 0 {
            continue;
        }
        for _ in 0..freq {
            table_symbol[position] = symbol as u16;
            position = (position + step) & mask;
            while position > high_threshold {
                position = (position + step) & mask;
            }
        }
    }

    // Phase 3: Build state table. tableU16[cumul[s]++] = tableSize + u.
    let mut state_table = vec![0u16; table_size];
    for u in 0..table_size {
        let s = table_symbol[u];
        let idx = cumul[usize::from(s)] as usize;
        state_table[idx] = (table_size + u) as u16;
        cumul[usize::from(s)] += 1;
    }

    // Phase 4: Build symbol transform table.
    let mut symbol_tt = vec![SymbolCompressionTransform::default(); max_sv1];
    let mut total = 0u32;
    for s in 0..max_sv1 {
        match norm[s] {
            0 => {
                symbol_tt[s].delta_nb_bits =
                    ((u32::from(table_log) + 1) << 16) - (1u32 << table_log);
            }
            -1 | 1 => {
                symbol_tt[s].delta_nb_bits = (u32::from(table_log) << 16) - (1u32 << table_log);
                symbol_tt[s].delta_find_state = total as i32 - 1;
                total += 1;
            }
            n => {
                let n_u32 = n as u32;
                let max_bits_out = u32::from(table_log) - (n_u32 - 1).ilog2();
                let min_state_plus = n_u32 << max_bits_out;
                symbol_tt[s].delta_nb_bits = (max_bits_out << 16) - min_state_plus;
                symbol_tt[s].delta_find_state = total as i32 - n_u32 as i32;
                total += n_u32;
            }
        }
    }

    Ok(CTable {
        table_log,
        max_symbol_value,
        state_table,
        symbol_tt,
    })
}

/// Write the probability-table header (`NCount`) to `out`. Ported from
/// C's `FSE_writeNCount_generic`.
///
/// Returns the number of bytes written.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on invalid input or buffer overflow.
pub fn write_ncount(
    out: &mut Vec<u8>,
    norm: &[i16],
    max_symbol_value: u8,
    table_log: u8,
) -> Result<usize, ZstdError> {
    let start_len = out.len();
    let table_size = 1u32 << table_log;
    let mut bit_stream: u32 = 0;
    let mut bit_count: u32 = 0;
    let mut symbol = 0u8;
    let alphabet_size = usize::from(max_symbol_value) + 1;
    let mut previous_is_0 = false;
    let mut remaining: i32 = table_size as i32 + 1;
    let mut threshold: i32 = table_size as i32;
    let mut nb_bits: i32 = table_log as i32 + 1;

    // Table size field: 4 bits.
    bit_stream |= u32::from(table_log - FSE_MIN_ACCURACY_LOG) << bit_count;
    bit_count += 4;

    let flush = |out: &mut Vec<u8>, bs: &mut u32, bc: &mut u32| {
        if *bc > 16 {
            out.push(*bs as u8);
            out.push((*bs >> 8) as u8);
            *bs >>= 16;
            *bc -= 16;
        }
    };

    while (symbol as usize) < alphabet_size && remaining > 1 {
        if previous_is_0 {
            let mut start = symbol;
            while (symbol as usize) < alphabet_size && norm[symbol as usize] == 0 {
                symbol += 1;
            }
            if symbol as usize == alphabet_size {
                return Err(ZstdError::Corrupt {
                    reason: "FSE writeNCount: bad distribution".into(),
                });
            }
            while symbol >= start + 24 {
                start += 24;
                bit_stream |= 0xFFFF << bit_count;
                out.push(bit_stream as u8);
                out.push((bit_stream >> 8) as u8);
                bit_stream >>= 16;
            }
            while symbol >= start + 3 {
                start += 3;
                bit_stream |= 3 << bit_count;
                bit_count += 2;
            }
            bit_stream |= u32::from(symbol - start) << bit_count;
            bit_count += 2;
            flush(out, &mut bit_stream, &mut bit_count);
        }

        let count = norm[symbol as usize];
        symbol += 1;
        let max = (2 * threshold - 1) - remaining;
        remaining -= if count < 0 {
            -count as i32
        } else {
            count as i32
        };
        let mut count_val = count as i32 + 1; // +1 for wire format
        if count_val >= threshold {
            count_val += max;
        }
        bit_stream |= (count_val as u32) << bit_count;
        bit_count += nb_bits as u32;
        if count_val < max {
            bit_count -= 1; // one fewer bit for the short encoding
        }

        previous_is_0 = count_val == 1;
        if remaining < 1 {
            return Err(ZstdError::Corrupt {
                reason: "FSE writeNCount: remaining < 1".into(),
            });
        }
        while remaining < threshold {
            nb_bits -= 1;
            threshold >>= 1;
        }
        flush(out, &mut bit_stream, &mut bit_count);
    }

    if remaining != 1 {
        return Err(ZstdError::Corrupt {
            reason: "FSE writeNCount: remaining != 1".into(),
        });
    }

    // Flush remaining bits — C writes 2 bytes then advances by
    // ceil(bitCount/8); with a Vec we must push exactly that many.
    if bit_count > 0 {
        let n_bytes = bit_count.div_ceil(8) as usize;
        let bytes = bit_stream.to_le_bytes();
        out.extend_from_slice(&bytes[..n_bytes]);
    }

    Ok(out.len() - start_len)
}

// ── Bit writer (BIT_CStream) ───────────────────────────────────────────

/// Forward bit writer matching C's `BIT_CStream_t`. Accumulates bits
/// at the LOW end of a u64 container and flushes whole bytes to the
/// output buffer (LE order).
#[derive(Debug)]
pub struct BitCStream<'a> {
    out: &'a mut Vec<u8>,
    start_len: usize,
    container: u64,
    bit_pos: u32, // 0..56
}

impl<'a> BitCStream<'a> {
    pub fn new(out: &'a mut Vec<u8>) -> Self {
        let start_len = out.len();
        Self {
            out,
            start_len,
            container: 0,
            bit_pos: 0,
        }
    }

    /// Add `nbBits` from the low end of `value`. Up to 31 bits per call.
    pub fn add_bits(&mut self, value: u64, nb_bits: u32) {
        debug_assert!(
            self.bit_pos + nb_bits <= 64,
            "BitCStream overflow: bit_pos {} + nb_bits {} > 64",
            self.bit_pos,
            nb_bits
        );
        let mask = if nb_bits >= 32 {
            u32::MAX
        } else {
            (1u32 << nb_bits) - 1
        };
        self.container |= (value & u64::from(mask)) << self.bit_pos;
        self.bit_pos += nb_bits;
    }

    /// Flush whole bytes to the output, keeping 0..7 bits in the container.
    pub fn flush(&mut self) {
        let nb_bytes = (self.bit_pos >> 3) as usize;
        let bytes = self.container.to_le_bytes();
        self.out.extend_from_slice(&bytes[..nb_bytes]);
        self.bit_pos &= 7;
        self.container >>= nb_bytes * 8;
    }

    /// Finalize: add the 1-bit end mark, flush, and return the total
    /// bytes written. Returns 0 on overflow (impossible for our callers
    /// since we grow `out` dynamically).
    #[must_use]
    pub fn close(mut self) -> usize {
        self.add_bits(1, 1); // end mark
        self.flush();
        if self.bit_pos > 0 {
            self.out.push(self.container as u8);
        }
        self.out.len() - self.start_len
    }
}

// ── FSE state encode ────────────────────────────────────────────────────

/// FSE encoder state. Tracks the current state value through the
/// encoding loop.
#[derive(Clone, Copy, Debug)]
pub struct CState {
    /// Current state value (always in `[tableSize, 2*tableSize)`).
    value: u32,
    state_log: u8,
}

impl CState {
    /// Initialize state to the baseline for `symbol` (the first symbol
    /// to encode = the last to decode). Matches C's `FSE_initCState2`.
    #[must_use]
    pub fn init2(table: &CTable, symbol: u8) -> Self {
        let s_tt = table.symbol_tt[usize::from(symbol)];
        let nb_bits_out = (s_tt.delta_nb_bits + (1 << 15)) >> 16;
        let value = (nb_bits_out << 16) - s_tt.delta_nb_bits;
        let idx = i64::from(value >> nb_bits_out) + i64::from(s_tt.delta_find_state);
        let value = u32::from(table.state_table[idx as usize]);
        Self {
            value,
            state_log: table.table_log,
        }
    }

    /// Encode one symbol: emit nbBits, update state. Matches C's
    /// `FSE_encodeSymbol`.
    pub fn encode(&mut self, bitc: &mut BitCStream<'_>, table: &CTable, symbol: u8) {
        let s_tt = table.symbol_tt[usize::from(symbol)];
        let nb_bits_out = (self.value + s_tt.delta_nb_bits) >> 16;
        bitc.add_bits(u64::from(self.value), nb_bits_out);
        let idx = i64::from(self.value >> nb_bits_out) + i64::from(s_tt.delta_find_state);
        self.value = u32::from(table.state_table[idx as usize]);
    }

    /// Flush the final state. Matches C's `FSE_flushCState`.
    pub fn flush(&self, bitc: &mut BitCStream<'_>) {
        bitc.add_bits(u64::from(self.value), u32::from(self.state_log));
        bitc.flush();
    }
}

/// Encode `symbols` using `table` and append the bitstream to `out`.
/// Ported from C's `FSE_compress_usingCTable_generic`.
///
/// The symbols are processed in REVERSE order (last symbol first),
/// matching the decoder's forward read order. The bitstream uses
/// 2-state interleaving for throughput.
///
/// Returns the number of bytes appended to `out`.
pub fn compress_using_ctable(out: &mut Vec<u8>, symbols: &[u8], table: &CTable) -> usize {
    if symbols.len() <= 2 {
        return 0; // Too few to encode; caller should fall back to Raw.
    }

    let _start_len = out.len();
    let mut bitc = BitCStream::new(out);
    let mut s1: CState;
    let mut s2: CState;

    // Initialize from the end of the input. The last symbol initializes
    // one state, the second-to-last initializes the other. Odd-length
    // inputs encode one extra symbol here to align to mod-4.
    let mut ip = symbols.len();
    if symbols.len() & 1 != 0 {
        ip -= 1;
        s1 = CState::init2(table, symbols[ip]);
        ip -= 1;
        s2 = CState::init2(table, symbols[ip]);
        ip -= 1;
        s1.encode(&mut bitc, table, symbols[ip]);
        bitc.flush();
    } else {
        ip -= 1;
        s2 = CState::init2(table, symbols[ip]);
        ip -= 1;
        s1 = CState::init2(table, symbols[ip]);
    }

    // Encode 2 or 4 symbols per loop iteration (4 on 64-bit to reduce flushes).
    while ip > 0 {
        ip -= 1;
        s2.encode(&mut bitc, table, symbols[ip]);
        if ip == 0 {
            break;
        }
        ip -= 1;
        s1.encode(&mut bitc, table, symbols[ip]);
        bitc.flush();
    }

    // Flush final states in reverse declaration order: s2, then s1.
    s2.flush(&mut bitc);
    s1.flush(&mut bitc);

    bitc.close()
}

/// Top-level convenience: normalize, build `CTable`, write `NCount` +
/// bitstream into `out`. Returns total bytes written.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on any normalization or encoding
/// failure.
pub fn compress(
    out: &mut Vec<u8>,
    symbols: &[u8],
    max_symbol_value: u8,
    table_log: u8,
) -> Result<usize, ZstdError> {
    if symbols.len() <= 2 {
        return Ok(0);
    }

    // Count histogram.
    let mut count = vec![0u32; usize::from(max_symbol_value) + 1];
    let mut total = 0u64;
    for &s in symbols {
        if s > max_symbol_value {
            return Err(ZstdError::Corrupt {
                reason: format!("FSE compress: symbol {s} > max {max_symbol_value}"),
            });
        }
        count[usize::from(s)] += 1;
        total += 1;
    }

    // Determine the actual max symbol (trailing zeros are elided).
    let mut actual_max = max_symbol_value;
    while actual_max > 0 && count[usize::from(actual_max)] == 0 {
        actual_max -= 1;
    }

    let norm = normalize_count(table_log, &count, total, actual_max, true)?;
    if norm.is_empty() {
        // RLE case — write an RLE CTable and empty bitstream.
        let _ = CTable::build_rle(actual_max);
        return Ok(0);
    }

    let ctable = build_ctable(&norm, actual_max, table_log)?;
    let start = out.len();
    write_ncount(out, &norm, actual_max, table_log)?;
    compress_using_ctable(out, symbols, &ctable);
    Ok(out.len() - start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fse::{decode_stream, read_fse_table};

    #[test]
    fn normalize_uniform_distribution() {
        let count = vec![8, 8, 8, 8]; // 4 symbols, each appearing 8 times.
        let norm = normalize_count(5, &count, 32, 3, true).expect("normalize");
        let positive_sum: i32 = norm.iter().filter(|&&x| x > 0).map(|&x| x as i32).sum();
        assert_eq!(positive_sum, 32);
    }

    #[test]
    fn build_ctable_from_normalized() {
        let norm = vec![8i16, 8, 8, 8];
        let ctable = build_ctable(&norm, 3, 5).expect("build");
        assert_eq!(ctable.table_log(), 5);
        assert_eq!(ctable.state_table.len(), 32);
    }

    #[test]
    fn optimal_table_log_basic() {
        // For 1000 symbols, maxSym=255, maxLog=6:
        // min_bits_src = highbit(1000)+1 = 10 → tableLog = min(10, 9) = 9.
        let tl = optimal_table_log(6, 1000, 255);
        assert!(tl >= FSE_MIN_ACCURACY_LOG);
        assert!(tl <= FSE_MAX_ACCURACY_LOG);

        // For a small stream with few symbols, tableLog stays small.
        let tl2 = optimal_table_log(6, 100, 5);
        assert!(tl2 >= FSE_MIN_ACCURACY_LOG);
        assert!(tl2 <= 6);
    }

    #[test]
    fn round_trip_simple_stream() {
        // Encode a short symbol stream and verify the decoder reproduces it.
        let symbols: Vec<u8> = (0..32).map(|i| (i % 4) as u8).collect();
        let count = vec![8u32, 8, 8, 8];
        let norm = normalize_count(5, &count, 32, 3, true).expect("normalize");
        let ctable = build_ctable(&norm, 3, 5).expect("build");

        let mut out = Vec::new();
        write_ncount(&mut out, &norm, 3, 5).expect("writeNCount");
        let ncount_len = out.len();
        compress_using_ctable(&mut out, &symbols, &ctable);

        // Decode: split NCount from bitstream.
        let (dtable, consumed) = read_fse_table(&out).expect("read table");
        assert_eq!(consumed, ncount_len);
        let bitstream = &out[consumed..];
        let decoded = decode_stream(&dtable, bitstream, symbols.len()).expect("decode");
        assert_eq!(decoded, symbols);
    }

    #[test]
    fn ncount_round_trip_large_alphabet() {
        // Exercise the NCount write+read pair with maxSymbolValue=11,
        // tableLog=6 (the configuration used for Huffman weight encoding).
        let count = vec![40u32, 60, 35, 25, 20, 15, 10, 10, 5, 5, 5, 5];
        let total: u64 = count.iter().map(|&c| u64::from(c)).sum();
        let norm = normalize_count(6, &count, total, 11, true).expect("normalize");
        let sum: i32 = norm
            .iter()
            .map(|&n| {
                if n > 0 {
                    n as i32
                } else if n < 0 {
                    1
                } else {
                    0
                }
            })
            .sum();
        assert_eq!(
            sum, 64,
            "normalized counts must sum to tableSize=64, got {norm:?}"
        );

        let mut out = Vec::new();
        write_ncount(&mut out, &norm, 11, 6).expect("writeNCount");

        let (dtable, consumed) = read_fse_table(&out).expect("read table");
        assert_eq!(consumed, out.len());
        let _ = dtable;
    }

    #[test]
    fn round_trip_of_predefined_distribution() {
        // OF_DEFAULT_NORM has -1 (low-prob) entries at indices 24-28.
        // Verify the FSE encoder handles these correctly.
        let of_norm: Vec<i16> = vec![
            1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1,
            -1,
        ];
        let ctable = build_ctable(&of_norm, 28, 5).expect("build OF ctable");
        let symbols: Vec<u8> = vec![3, 3, 3, 2, 4, 3, 3, 2, 5, 3, 3, 3, 2, 4, 3, 3];

        let mut out = Vec::new();
        write_ncount(&mut out, &of_norm, 28, 5).expect("writeNCount");
        let ncount_len = out.len();
        compress_using_ctable(&mut out, &symbols, &ctable);

        let (dtable, consumed) = read_fse_table(&out).expect("read table");
        assert_eq!(consumed, ncount_len);
        let bitstream = &out[consumed..];
        let decoded = decode_stream(&dtable, bitstream, symbols.len()).expect("decode");
        assert_eq!(decoded, symbols);
    }
}
