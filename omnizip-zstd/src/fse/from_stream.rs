//! FSE-from-stream reader — reads a normalized probability distribution
//! from the wire and builds an FSE decode [`Table`].
//!
//! Verified against `FSE_readNCount_body` in
//! `~/src/external/zstd/lib/common/entropy_common.c:42-187`.
//!
//! ## Wire format
//!
//! The distribution is encoded as a bit-packed stream:
//!
//! 1. **tableLog** (4 bits): `accuracy_log - FSE_MIN_TABLELOG`. Must be
//!    `≤ FSE_MAX_ACCURACY_LOG - FSE_MIN_TABLELOG`.
//! 2. **Per-symbol counts**: each symbol's count is encoded using
//!    `(tableLog + 1)` bits. Special encodings handle runs of zero-count
//!    symbols (3-repeat blocks) and counts above the running average.
//! 3. The encoder stops once `remaining == 1`.
//!
//! After reading, the count array is fed into [`Table::build`] to
//! construct the decode table.

#![forbid(unsafe_code)]

use crate::constants::{FSE_MAX_ACCURACY_LOG, FSE_MIN_ACCURACY_LOG};
use crate::fse::Table;
use crate::ZstdError;

/// Maximum alphabet size FSE can encode. ZSTD's largest alphabet is
/// 255 (Huffman weights).
const MAX_SYMBOL_VALUE: usize = 255;

/// Reader state for the bit-packed FSE table header.
struct BitReader<'a> {
    src: &'a [u8],
    byte_pos: usize,
    /// Current 32-bit window onto `src[byte_pos..byte_pos+4]`.
    bit_window: u32,
    /// Number of valid low bits in `bit_window`.
    bit_count: u32,
}

impl<'a> BitReader<'a> {
    fn new(src: &'a [u8]) -> Result<Self, ZstdError> {
        if src.len() < 4 {
            return Err(ZstdError::Corrupt {
                reason: "FSE bitstream needs at least 4 bytes".into(),
            });
        }
        let bit_window = u32::from_le_bytes([src[0], src[1], src[2], src[3]]);
        Ok(Self {
            src,
            byte_pos: 0,
            bit_window,
            bit_count: 0,
        })
    }

    /// Advance `bit_count >> 3` bytes; reload the 32-bit window so the
    /// low `bit_count & 7` bits remain valid.
    fn refill(&mut self, extra_bits: u32) -> Result<(), ZstdError> {
        self.bit_count += extra_bits;
        let advance = (self.bit_count >> 3) as usize;
        if self.byte_pos + advance + 4 > self.src.len() {
            // Trailing run: clamp to the last 4 bytes of the stream,
            // as the C reference does (`ip = iend - 4`).
            if self.src.len() < 4 {
                return Err(ZstdError::Corrupt {
                    reason: "FSE bitstream truncated mid-refill".into(),
                });
            }
            let new_pos = self.src.len() - 4;
            // Adjust bit_count so we don't lose the still-pending bits.
            let consumed = (new_pos as i64 - self.byte_pos as i64) * 8;
            self.bit_count = (self.bit_count as i64 - consumed) as u32 & 31;
            self.byte_pos = new_pos;
        } else {
            self.byte_pos += advance;
            self.bit_count &= 7;
        }
        let bytes = &self.src[self.byte_pos..self.byte_pos + 4];
        self.bit_window = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
            >> self.bit_count;
        Ok(())
    }

    /// Read `n` low bits and advance.
    #[allow(dead_code)]
    fn read_bits(&mut self, n: u32) -> Result<u32, ZstdError> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.bit_window & ((1u32 << n) - 1);
        self.refill(n)?;
        Ok(v)
    }

    /// Drop `n` low bits without inspecting them.
    fn skip_bits(&mut self, n: u32) -> Result<(), ZstdError> {
        if n == 0 {
            return Ok(());
        }
        self.refill(n)
    }

    /// Peek the low 32 bits without advancing.
    const fn peek(&self) -> u32 {
        self.bit_window
    }

    /// Number of bytes consumed from `src` (rounded up to whole bytes).
    fn bytes_consumed(&self) -> usize {
        self.byte_pos + ((self.bit_count + 7) >> 3) as usize
    }
}

/// Read a normalized probability distribution from `src` and return
/// the FSE decode [`Table`] plus the number of bytes consumed.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] on truncation, invalid tableLog,
/// or `remaining != 1` at the end (the distribution doesn't sum to
/// `1 << tableLog`).
pub fn read_fse_table(src: &[u8]) -> Result<(Table, usize), ZstdError> {
    let mut br = BitReader::new(src)?;

    // tableLog: 4-bit field, biased by FSE_MIN_ACCURACY_LOG.
    let table_log_raw = (br.peek() & 0xF) as u8;
    let table_log = table_log_raw.checked_add(FSE_MIN_ACCURACY_LOG).ok_or_else(|| ZstdError::Corrupt {
        reason: format!("FSE tableLog overflow: {table_log_raw} + {FSE_MIN_ACCURACY_LOG}"),
    })?;
    if table_log > FSE_MAX_ACCURACY_LOG {
        return Err(ZstdError::Corrupt {
            reason: format!("FSE tableLog {table_log} exceeds max {FSE_MAX_ACCURACY_LOG}"),
        });
    }
    br.skip_bits(4)?;

    let mut remaining: i32 = (1 << table_log) + 1;
    let mut threshold: i32 = 1 << table_log;
    let mut nb_bits: u32 = u32::from(table_log) + 1;

    let mut counts = vec![0i16; MAX_SYMBOL_VALUE + 1];
    let mut charnum: usize = 0;
    let mut previous0 = false;

    loop {
        if previous0 {
            // Count 2-bit repeat codes (0b11 = "skip 3 more zeros").
            let inverted = !br.peek() | 0x8000_0000;
            let mut repeats = inverted.trailing_zeros() >> 1;
            while repeats >= 12 {
                charnum = charnum.saturating_add(3 * 12).min(MAX_SYMBOL_VALUE + 1);
                br.skip_bits(2 * 12)?;
                repeats = (!br.peek() | 0x8000_0000).trailing_zeros() >> 1;
            }
            charnum = charnum
                .saturating_add(3 * repeats as usize)
                .min(MAX_SYMBOL_VALUE + 1);
            br.skip_bits(2 * repeats)?;

            // Final repeat which isn't 0b11.
            let tail = br.peek() & 3;
            charnum = charnum
                .saturating_add(tail as usize)
                .min(MAX_SYMBOL_VALUE + 1);
            br.skip_bits(2)?;

            if charnum > MAX_SYMBOL_VALUE {
                return Err(ZstdError::Corrupt {
                    reason: format!("FSE table: charnum {charnum} exceeds alphabet"),
                });
            }
            previous0 = false;
            if charnum > MAX_SYMBOL_VALUE {
                break;
            }
            continue;
        }

        let max = (2 * threshold - 1) - remaining;
        let masked = br.peek() as i32 & (threshold - 1);
        let count: i32;
        if masked < max {
            count = masked;
            br.skip_bits(nb_bits - 1)?;
        } else {
            let raw = br.peek() as i32 & (2 * threshold - 1);
            let mut c = raw;
            if c >= threshold {
                c -= max;
            }
            count = c;
            br.skip_bits(nb_bits)?;
        }

        let actual_count = count - 1;
        if actual_count >= 0 {
            remaining -= actual_count;
        } else {
            remaining += actual_count; // actual_count == -1
        }
        if charnum < counts.len() {
            counts[charnum] = actual_count as i16;
        }
        charnum += 1;
        previous0 = actual_count == 0;

        if remaining < threshold {
            if remaining <= 1 {
                break;
            }
            // highbit32(remaining) + 1
            let high = (remaining as u32).ilog2();
            nb_bits = high + 1;
            threshold = 1 << high;
        }
        if charnum > MAX_SYMBOL_VALUE {
            break;
        }
    }

    if remaining != 1 {
        return Err(ZstdError::Corrupt {
            reason: format!("FSE table: remaining={remaining} (expected 1)"),
        });
    }

    counts.truncate(charnum);
    if counts.is_empty() {
        return Err(ZstdError::Corrupt {
            reason: "FSE table: empty alphabet".into(),
        });
    }

    let table = Table::build(&counts, table_log)?;
    let consumed = br.bytes_consumed().min(src.len());
    Ok((table, consumed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_header_is_corrupt() {
        assert!(read_fse_table(&[0, 1, 2]).is_err());
    }

    #[test]
    fn reject_oversized_table_log() {
        // 4-bit field = 0xF (15) → tableLog = 15 + 5 = 20 > FSE_MAX_ACCURACY_LOG (15).
        let src = [0xF, 0, 0, 0];
        assert!(read_fse_table(&src).is_err());
    }
}
