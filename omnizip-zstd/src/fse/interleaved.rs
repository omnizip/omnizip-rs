//! 2-state interleaved FSE decoder, matching C's
//! `FSE_decompress_usingDTable_generic` (tail loop only — the 4-symbol
//! fast loop is a performance optimisation that produces the same output).
//!
//! Verified against
//! `~/src/external/zstd/lib/common/fse_decompress.c:173-236` and
//! `~/src/external/zstd/lib/common/fse.h:517-549`.
//!
//! ## Termination
//!
//! The C decoder loops until `BIT_reloadDStream` returns
//! `BIT_DStream_overflow` (bitsConsumed > 64). After overflow, it decodes
//! one final symbol from the other state (the symbol comes from the table
//! lookup; the bit reads are irrelevant since we break immediately).
//!
//! Termination is NOT stream exhaustion — the decoder intentionally reads
//! past the logical end of the bitstream, relying on the container-based
//! overflow check to stop at the right point.

#![forbid(unsafe_code)]

use super::bitstream::{BitStream, ReloadStatus};
use crate::fse::Table;
use crate::ZstdError;

/// Decode a 2-state interleaved FSE stream. Produces symbols until the
/// bitstream overflows (matching C's termination semantics).
///
/// `max_output` caps the decoded length to prevent runaway loops on
/// corrupt input.
pub fn decode_stream(
    table: &Table,
    bitstream_bytes: &[u8],
    max_output: usize,
) -> Result<Vec<u8>, ZstdError> {
    let mut br = BitStream::new(bitstream_bytes);
    let accuracy_log = u32::from(table.accuracy_log());

    // Init: state1 reads accuracy_log bits, reload, state2 reads
    // accuracy_log bits, reload. Matches C's FSE_initDState which calls
    // BIT_readBits then BIT_reloadDStream internally.
    let mut s1 = br.read_bits(accuracy_log);
    br.reload();
    let mut s2 = br.read_bits(accuracy_log);
    br.reload();

    let mut out: Vec<u8> = Vec::with_capacity(max_output);

    loop {
        if out.len() >= max_output {
            return Err(ZstdError::Corrupt {
                reason: format!("FSE decode exceeded max_output {max_output}"),
            });
        }
        // Decode state1: read nbBits, update state.
        let e1 = table.state(usize::try_from(s1).unwrap_or(0));
        let extra1 = br.read_bits(u32::from(e1.num_bits));
        out.push(e1.symbol);
        s1 = e1.baseline + extra1;

        if br.reload_status() == ReloadStatus::Overflow {
            // One final state2 symbol. The bits read here are garbage
            // (stream is overflowed), but the symbol comes from the
            // table lookup and is correct.
            if out.len() >= max_output {
                return Err(ZstdError::Corrupt {
                    reason: format!("FSE decode exceeded max_output {max_output}"),
                });
            }
            let e2 = table.state(usize::try_from(s2).unwrap_or(0));
            out.push(e2.symbol);
            break;
        }

        if out.len() >= max_output {
            return Err(ZstdError::Corrupt {
                reason: format!("FSE decode exceeded max_output {max_output}"),
            });
        }
        // Decode state2.
        let e2 = table.state(usize::try_from(s2).unwrap_or(0));
        let extra2 = br.read_bits(u32::from(e2.num_bits));
        out.push(e2.symbol);
        s2 = e2.baseline + extra2;

        if br.reload_status() == ReloadStatus::Overflow {
            // One final state1 symbol.
            if out.len() >= max_output {
                return Err(ZstdError::Corrupt {
                    reason: format!("FSE decode exceeded max_output {max_output}"),
                });
            }
            let e1b = table.state(usize::try_from(s1).unwrap_or(0));
            out.push(e1b.symbol);
            break;
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stream_does_not_panic() {
        // An empty stream has no data. BitStream::new handles this by
        // setting bits_consumed such that reload overflows immediately.
        // We just verify no panic.
        let table = Table::build(&[32], 5).expect("build");
        let _ = decode_stream(&table, &[], 16);
    }
}
