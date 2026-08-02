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
    use crate::fse::decode_stream;
    use crate::fse::encoder::{
        build_ctable, compress_using_ctable, normalize_count, optimal_table_log, write_ncount,
    };

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

    /// Regression test for FSE round-trip of Huffman-weight-style payloads.
    ///
    /// This exercises the full encode → `NCount` write → `NCount` read → bitstream
    /// decode pipeline with the exact configuration ZSTD uses for
    /// FSE-compressed Huffman weights: tableLog=6, maxSymbolValue=11, and
    /// the `use_low_prob_count = true` normalization flag.
    fn round_trip_huffman_weights(weights: &[u8]) {
        assert!(weights.len() > 2, "need at least 3 weights for FSE");
        let o_size = weights.len();

        // Build frequency counts for weight values 0..=11.
        let mut counts = [0u32; 12];
        for &w in weights {
            counts[usize::from(w)] += 1;
        }

        // tableLog=6 matches ZSTD's Huffman weight FSE configuration.
        let table_log = optimal_table_log(6, o_size, 11);

        // use_low_prob_count = true matches C's FSE_LOWPROB_SYM_DEFAULT.
        let total = o_size as u64;
        let norm = normalize_count(table_log, &counts, total, 11, true).expect("normalize");
        assert!(!norm.is_empty(), "RLE case — not FSE-compressible");

        let ctable = build_ctable(&norm, 11, table_log).expect("build ctable");

        // Write NCount header + FSE bitstream.
        let mut payload = Vec::new();
        write_ncount(&mut payload, &norm, 11, table_log).expect("writeNCount");
        let ncount_len = payload.len();
        let compressed = compress_using_ctable(&mut payload, weights, &ctable);
        assert!(compressed > 0, "FSE bitstream should not be empty");

        // Decode: split NCount from bitstream.
        let (dtable, consumed) = read_fse_table(&payload).expect("read FSE table");
        assert_eq!(
            consumed, ncount_len,
            "NCount consumed bytes mismatch: encoder wrote {ncount_len}, decoder consumed {consumed}"
        );

        // Verify the decoded table matches the encoded distribution.
        assert_eq!(
            dtable.accuracy_log(),
            table_log,
            "tableLog mismatch: encoder={table_log}, decoder={}",
            dtable.accuracy_log()
        );

        let bitstream = &payload[consumed..];
        let decoded = decode_stream(&dtable, bitstream, weights.len()).expect("decode stream");

        assert_eq!(
            decoded.len(),
            weights.len(),
            "decoded length {} != expected {}",
            decoded.len(),
            weights.len()
        );
        assert_eq!(
            decoded, weights,
            "FSE round-trip mismatch:\n  encoded weights: {weights:?}\n  decoded weights: {decoded:?}"
        );
    }

    #[test]
    fn fse_round_trip_huffman_weights_3211() {
        // Classic 4-symbol Huffman tree with weights [3, 2, 1, 1].
        round_trip_huffman_weights(&[3, 2, 1, 1]);
    }

    #[test]
    fn fse_round_trip_huffman_weights_larger_alphabet() {
        // A larger weight set resembling a real Huffman tree for a
        // 200+ symbol alphabet (the scenario that triggers FSE-compressed
        // weights instead of direct encoding).
        let weights: Vec<u8> = (0..200)
            .map(|i| {
                #[allow(clippy::match_same_arms)]
                match i % 7 {
                    0 => 1,
                    1 => 2,
                    2 => 3,
                    3 => 1,
                    4 => 2,
                    5 => 4,
                    _ => 1,
                }
            })
            .collect();
        round_trip_huffman_weights(&weights);
    }

    #[test]
    fn fse_round_trip_huffman_weights_with_zeros() {
        // Weights including zeros (absent symbols). The encoder skips
        // zeros in the frequency count but they still appear in the
        // weight array since the alphabet is full.
        let weights: Vec<u8> = vec![0, 0, 3, 2, 1, 1, 0, 0, 2, 1, 3, 0, 1];
        round_trip_huffman_weights(&weights);
    }

    #[test]
    fn fse_round_trip_huffman_weights_skewed() {
        // Highly skewed distribution — one dominant symbol, many rare.
        let mut weights: Vec<u8> = vec![11; 1];
        weights.extend(vec![1; 50]);
        weights.extend(vec![2; 30]);
        weights.extend(vec![3; 20]);
        round_trip_huffman_weights(&weights);
    }

    #[test]
    fn fse_round_trip_huffman_weights_many_zeros() {
        // 256-symbol alphabet with many absent (weight-0) symbols,
        // producing long zero runs in the NCount header. This exercises
        // the previous0 repeat-counting path that is the most likely
        // site of an FSE NCount decoder bug.
        let mut weights: Vec<u8> = vec![0; 256];
        // Place non-zero weights at specific positions to create
        // runs of zeros of varying lengths.
        weights[0] = 5;
        weights[1] = 3;
        weights[2] = 2;
        weights[3] = 1;
        weights[4] = 1;
        weights[50] = 4;
        weights[51] = 3;
        weights[100] = 2;
        weights[101] = 1;
        weights[200] = 3;
        weights[201] = 2;
        weights[202] = 1;
        weights[255] = 1;
        round_trip_huffman_weights(&weights);
    }

    #[test]
    fn fse_round_trip_huffman_weights_very_long_zero_run() {
        // Specifically craft a payload where the zero-run between
        // two non-zero weight groups is long enough to trigger the
        // `repeats >= 12` inner loop in the NCount decoder.
        let mut weights: Vec<u8> = vec![0; 200];
        weights[0] = 4;
        weights[1] = 3;
        weights[2] = 2;
        weights[3] = 1;
        // 196 zeros (indices 4..199) → 65+ repeat triples
        weights[199] = 5;
        weights[198] = 2;
        weights[197] = 1;
        round_trip_huffman_weights(&weights);
    }

    /// Regression test against a real zstd C library (v1.5.7)
    /// FSE-compressed Huffman weights payload.
    ///
    /// This payload was extracted from a zstd -3 compressed file and
    /// decoded by `HUF_readStats` in the C reference. It exercises:
    /// - `read_fse_table` (`NCount` header parsing)
    /// - `Table::build` (FSE decode table construction)
    /// - `decode_stream` (interleaved FSE bitstream decode)
    ///
    /// The payload has tableLog=5, maxSV=8, with low-probability symbols
    /// and zero-runs in the `NCount` header.
    #[test]
    fn fse_decode_matches_c_reference_real_payload() {
        // Tree byte (0x1e = 30 bytes of FSE data) + FSE NCount + bitstream.
        let header: [u8; 31] = [
            0x1e, 0x10, 0xd8, 0xda, 0x72, 0x0c, 0x03, 0xb8, 0xa2, 0x61, 0x70, 0x4d, 0x92, 0x3a,
            0x91, 0x6e, 0xa1, 0x26, 0x12, 0xd9, 0x6e, 0xa1, 0xa5, 0x95, 0xed, 0x16, 0x35, 0x0c,
            0x53, 0x91, 0x02,
        ];
        let fse_payload = &header[1..];
        let (table, consumed) = read_fse_table(fse_payload).expect("read FSE table");
        let bitstream = &fse_payload[consumed..];

        // The ACTUAL C reference output (from HUF_readStats in zstd 1.5.7).
        // Weight=2 appears at positions 142, 195, 212.
        // All other non-1/non-8 values follow the pattern from the FSE table.
        let c_ref: [u8; 255] = [
            8, 1, 1, 1, 1, 1, 1, 1, // 0-7
            1, 1, 1, 1, 1, 1, 1, 1, // 8-15
            1, 1, 1, 1, 1, 1, 1, 1, // 16-23
            1, 1, 1, 1, 1, 1, 1, 1, // 24-31
            1, 1, 1, 1, 1, 1, 1, 1, // 32-39
            1, 1, 1, 1, 1, 1, 1, 1, // 40-47
            1, 1, 1, 1, 1, 1, 1, 1, // 48-55
            1, 1, 1, 1, 1, 1, 1, 1, // 56-63
            1, 3, 3, 4, 3, 3, 4, 4, // 64-71
            4, 3, 4, 3, 4, 3, 3, 4, // 72-79
            4, 4, 3, 3, 3, 3, 4, 3, // 80-87
            3, 4, 4, 1, 1, 1, 1, 1, // 88-95
            1, 1, 1, 1, 1, 1, 1, 1, // 96-103
            1, 1, 1, 1, 1, 1, 1, 1, // 104-111
            1, 1, 1, 1, 1, 1, 1, 1, // 112-119
            1, 1, 1, 1, 1, 1, 1, 1, // 120-127
            1, 1, 1, 1, 1, 1, 1, 1, // 128-135
            1, 1, 1, 1, 1, 1, 2, 1, // 136-143 (142=2)
            1, 1, 1, 1, 1, 1, 1, 1, // 144-151
            1, 1, 1, 1, 1, 1, 1, 1, // 152-159
            1, 1, 1, 1, 1, 1, 1, 1, // 160-167
            1, 1, 1, 1, 1, 1, 1, 1, // 168-175
            1, 1, 1, 1, 1, 1, 1, 1, // 176-183
            1, 1, 1, 1, 1, 1, 1, 1, // 184-191
            1, 1, 1, 2, 1, 1, 1, 1, // 192-199 (195=2)
            1, 1, 1, 1, 1, 1, 1, 1, // 200-207
            1, 1, 1, 1, 2, 1, 1, 1, // 208-215 (212=2)
            1, 1, 1, 1, 1, 1, 1, 1, // 216-223
            1, 1, 1, 1, 1, 1, 1, 1, // 224-231
            1, 1, 1, 1, 1, 1, 1, 1, // 232-239
            1, 1, 1, 1, 1, 1, 1, 1, // 240-247
            1, 1, 1, 1, 1, 1, 1, // 248-254
        ];

        // Call decode_stream (the production decoder)
        let ds_out = decode_stream(&table, bitstream, 255).expect("decode_stream");

        assert_eq!(ds_out.len(), 255, "decoded weight count");
        assert_eq!(ds_out, c_ref, "FSE-decoded weights must match C reference");
    }
}
