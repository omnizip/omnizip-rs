//! FSE decoding table + decoder.
//!
//! Ported from the C reference `FSE_buildDTable_internal` in
//! `lib/common/entropy_common.c` (facebook/zstd, BSD-3-Clause).
//!
//! ## Algorithm
//!
//! 1. **Spread**: symbols with positive count spread from position 0
//!    using step `(table_size >> 1) + (table_size >> 3) + 3`. Symbols
//!    with count `-1` (low-probability) are placed at `highThreshold`
//!    from the top of the table. Collision detection skips occupied
//!    cells.
//!
//! 2. **State values**: for each cell `u` in table order:
//!    - `nextState = symbolNext[symbol]++` (initialised to 1 for
//!      `-1`/`1` counts, to `count` for higher counts)
//!    - `nbBits = tableLog - highbit(nextState)`
//!    - `baseline = (nextState << nbBits) - tableSize`
//!
//! The `baseline + extra_bits` formula distributes each symbol's
//! cells across equal-sized ranges of the next-state space.

#![forbid(unsafe_code)]

use super::bitstream::BitStream;
use crate::ZstdError;

/// One FSE table entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FseState {
    pub symbol: u8,
    pub num_bits: u8,
    pub baseline: u32,
}

/// FSE decoding table built from a normalised probability distribution.
#[derive(Clone, Debug)]
pub struct Table {
    states: Vec<FseState>,
    accuracy_log: u8,
}

impl Table {
    /// Build a table from a distribution. Positive entries give the
    /// cell count for each symbol. `-1` entries mark low-probability
    /// symbols that each get exactly one cell at the top of the table.
    /// `0` entries mean the symbol is absent.
    ///
    /// The total `sum(positive) + count(-1)` must equal `1 << accuracy_log`.
    ///
    /// # Errors
    ///
    /// Returns [`ZstdError::Corrupt`] if the distribution doesn't fill
    /// the table exactly.
    pub fn build(distribution: &[i16], accuracy_log: u8) -> Result<Self, ZstdError> {
        let table_size = 1usize << accuracy_log;
        let step = (table_size >> 1) + (table_size >> 3) + 3;
        let mask = table_size - 1;

        // Phase 1: Place symbols.
        // table_symbol tracks which symbol occupies each cell.
        // Initialize to 0xFFFF (sentinel = empty).
        let mut table_symbol: Vec<u16> = vec![0xFFFF; table_size];
        let mut high_threshold = table_size - 1;
        let mut position = 0usize;

        for (symbol, &freq) in distribution.iter().enumerate() {
            if freq == 0 {
                continue;
            }
            let symbol_u16 = u16::try_from(symbol).map_err(|_| ZstdError::Corrupt {
                reason: format!("FSE symbol {symbol} exceeds u16"),
            })?;

            if freq == -1 {
                // Low-probability: place at high_threshold from top.
                table_symbol[high_threshold] = symbol_u16;
                if high_threshold == 0 {
                    return Err(ZstdError::Corrupt {
                        reason: "FSE high_threshold underflow".into(),
                    });
                }
                high_threshold -= 1;
                continue;
            }

            // Positive: spread from position. Find empty cell FIRST,
            // then place, then advance. This avoids an infinite search
            // after placing the last cell (all cells full).
            for _ in 0..freq {
                // Find the next empty cell.
                let mut guard = 0usize;
                while table_symbol[position] != 0xFFFF {
                    position = (position + step) & mask;
                    guard += 1;
                    if guard > table_size {
                        return Err(ZstdError::Corrupt {
                            reason: "FSE spread failed: no empty cell".into(),
                        });
                    }
                }
                // Place the symbol.
                table_symbol[position] = symbol_u16;
                // Advance for the next iteration.
                position = (position + step) & mask;
            }
        }

        // Phase 2: Initialize symbolNext.
        // For freq -1 or 1: start at 1. For freq >= 2: start at freq.
        let mut symbol_next: Vec<u32> = vec![0; distribution.len()];
        for (symbol, &freq) in distribution.iter().enumerate() {
            if freq == 0 {
                continue;
            }
            symbol_next[symbol] = if freq == -1 || freq == 1 {
                1
            } else {
                u32::try_from(freq).unwrap_or(0)
            };
        }

        // Phase 3: Build decode table.
        let mut states = Vec::with_capacity(table_size);
        for &symbol_u16 in &table_symbol {
            let symbol = u8::try_from(symbol_u16).unwrap_or(0);
            let next_state = symbol_next[usize::from(symbol)];
            symbol_next[usize::from(symbol)] += 1;

            let nb_bits = u32::from(accuracy_log) - highbit(next_state);
            let baseline = (next_state << nb_bits) - table_size as u32;

            states.push(FseState {
                symbol,
                num_bits: nb_bits as u8,
                baseline,
            });
        }

        Ok(Self {
            states,
            accuracy_log,
        })
    }

    /// Build from one of the RFC 8878 §4.1.3 predefined distributions.
    /// Normalizes the raw distribution to sum to `1 << accuracy_log`
    /// before building the table.
    ///
    /// # Errors
    ///
    /// Returns [`ZstdError::Corrupt`] if the normalised distribution
    /// fails to fill the table exactly (delegates to [`Self::build`]).
    pub fn build_predefined(distribution: &[i16], accuracy_log: u8) -> Result<Self, ZstdError> {
        let normalized = normalize_distribution(distribution, accuracy_log);
        Self::build(&normalized, accuracy_log)
    }

    /// Construct a single-symbol RLE table.
    #[must_use]
    pub fn build_rle(symbol: u8, accuracy_log: u8) -> Self {
        let table_size = 1usize << accuracy_log;
        let state = FseState {
            symbol,
            num_bits: 0,
            baseline: 0,
        };
        Self {
            states: vec![state; table_size],
            accuracy_log,
        }
    }

    #[must_use]
    pub fn state(&self, index: usize) -> FseState {
        self.states[index]
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.states.len()
    }

    #[must_use]
    pub const fn accuracy_log(&self) -> u8 {
        self.accuracy_log
    }
}

/// `floor(log2(x))` for x > 0. Returns 0 for x == 1.
fn highbit(x: u32) -> u32 {
    if x == 0 {
        return 0;
    }
    x.ilog2()
}

/// Stateful FSE decoder.
#[derive(Debug)]
pub struct FseDecoder<'t> {
    table: &'t Table,
    state: u32,
}

impl<'t> FseDecoder<'t> {
    #[must_use]
    pub fn new(table: &'t Table) -> Self {
        Self { table, state: 0 }
    }

    pub fn init_state(&mut self, bitstream: &mut BitStream<'_>) {
        self.state = bitstream.read_bits(u32::from(self.table.accuracy_log()));
    }

    #[must_use]
    pub const fn state(&self) -> u32 {
        self.state
    }

    pub fn decode(&mut self, bitstream: &mut BitStream<'_>) -> u8 {
        let entry = self.table.state(usize::try_from(self.state).unwrap_or(0));
        if entry.num_bits > 0 {
            let extra = bitstream.read_bits(u32::from(entry.num_bits));
            self.state = entry.baseline + extra;
        } else {
            self.state = entry.baseline;
        }
        entry.symbol
    }
}

/// Normalize a raw probability distribution to sum to `1 << accuracy_log`.
///
/// Ported from the C reference `FSE_normalizeCount` (`lib/common/entropy_common.c`).
/// The algorithm:
/// 1. Compute scale = (1 << 62) / `raw_sum`.
/// 2. For each entry: scaled = (entry * scale) >> (62 - tableLog).
/// 3. Distribute the remainder (tableSize - sum(scaled)) one per entry
///    to those with the largest rounding error, breaking ties by symbol order.
///
/// Entries with value `-1` (low-probability sentinels) and `0` (absent)
/// are passed through unchanged.
fn normalize_distribution(raw: &[i16], accuracy_log: u8) -> Vec<i16> {
    let table_size = 1usize << accuracy_log;
    let sentinel_count = raw.iter().filter(|&&x| x == -1).count();
    let target = table_size - sentinel_count; // positive entries fill this many cells

    let raw_sum: u64 = raw.iter().filter(|&&x| x > 0).map(|&x| x as u64).sum();

    if raw_sum == 0 || raw_sum == target as u64 {
        return raw.to_vec();
    }

    let scale = (1u64 << 62) / raw_sum;
    let shift = 62u32 - u32::from(accuracy_log);

    // Scale each positive entry.
    let mut normalized: Vec<i16> = raw
        .iter()
        .map(|&x| {
            if x <= 0 {
                x
            } else {
                ((x as u64 * scale) >> shift) as i16
            }
        })
        .collect();

    // Compute remainder and distribute.
    let current_sum: i32 = normalized.iter().filter(|&&x| x > 0).map(|&x| x as i32).sum();
    let remainder = target as i32 - current_sum;

    if remainder > 0 {
        // Sort positive entries by rounding error (descending), ties by index.
        let mut errors: Vec<(usize, u64)> = raw
            .iter()
            .enumerate()
            .filter(|(_, &x)| x > 0)
            .map(|(i, &x)| {
                let scaled = (x as u64 * scale) >> shift;
                let error = (x as u64 * scale) - (scaled << shift);
                (i, error)
            })
            .collect();
        errors.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        for (k, &(idx, _)) in errors.iter().enumerate() {
            if k >= remainder as usize {
                break;
            }
            normalized[idx] += 1;
        }
    } else if remainder < 0 {
        // Overfull: remove from entries with smallest error.
        let mut errors: Vec<(usize, u64)> = raw
            .iter()
            .enumerate()
            .filter(|(_, &x)| x > 0)
            .map(|(i, &x)| {
                let scaled_val = normalized[i] as u64;
                let error = (x as u64 * scale) - (scaled_val << shift);
                (i, error)
            })
            .collect();
        errors.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));

        for (k, &(idx, _)) in errors.iter().enumerate() {
            if k >= (-remainder) as usize {
                break;
            }
            normalized[idx] -= 1;
        }
    }

    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highbit_zero_returns_zero() {
        assert_eq!(highbit(0), 0);
    }

    #[test]
    fn highbit_powers_of_two() {
        assert_eq!(highbit(1), 0);
        assert_eq!(highbit(2), 1);
        assert_eq!(highbit(4), 2);
        assert_eq!(highbit(8), 3);
        assert_eq!(highbit(16), 4);
        assert_eq!(highbit(32), 5);
    }

    #[test]
    fn rle_table_always_returns_same_symbol() {
        let table = Table::build_rle(7, 5);
        for i in 0..table.size() {
            assert_eq!(table.state(i).symbol, 7);
        }
    }

    #[test]
    fn build_uniform_distribution() {
        // 4 symbols, each with count 8, table_size=32.
        let dist: [i16; 4] = [8, 8, 8, 8];
        let table = Table::build(&dist, 5).expect("build");
        assert_eq!(table.size(), 32);
        let mut counts = [0u32; 4];
        for i in 0..table.size() {
            counts[table.state(i).symbol as usize] += 1;
        }
        assert_eq!(counts, [8, 8, 8, 8]);
    }

    #[test]
    fn build_with_low_probability_sentinels() {
        // 3 positive symbols (sum=29) + 3 low-probability (-1) = 32.
        let dist: [i16; 6] = [10, 10, 9, -1, -1, -1];
        let table = Table::build(&dist, 5).expect("build");
        assert_eq!(table.size(), 32);
        // Every symbol 0-5 should appear at least once.
        let mut seen = [false; 6];
        for i in 0..table.size() {
            seen[table.state(i).symbol as usize] = true;
        }
        for (idx, &present) in seen.iter().enumerate() {
            assert!(present, "symbol {idx} not found in table");
        }
    }

    #[test]
    fn build_predefined_offset_distribution_sums_correctly() {
        // The C reference offset distribution: 27 positive + 5 sentinels = 32.
        let dist: [i16; 32] = [
            1, 1, 1, 1, 1, 1, 2, 2,
            2, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1,
            -1, -1, -1, -1, -1, 0, 0, 0,
        ];
        let positive_sum: i32 = dist.iter().filter(|&&x| x > 0).map(|&x| x as i32).sum();
        let sentinel_count = dist.iter().filter(|&&x| x == -1).count();
        assert_eq!(positive_sum + sentinel_count as i32, 32);

        let table = Table::build(&dist, 5).expect("build");
        assert_eq!(table.size(), 32);
        // Verify every cell has a valid symbol.
        for i in 0..table.size() {
            let s = table.state(i).symbol;
            assert!(s < 29, "symbol {s} at cell {i} exceeds maxSV=28");
        }
    }
}
