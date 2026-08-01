//! Shared helpers for tree-shaped bit-model coders.
//!
//! Both the length and distance coders use binary trees of [`BitModel`]
//! to encode/decode symbols of a fixed bit-width. The Ruby implements
//! `decode_tree` / `decode_reverse_tree` as private methods on each
//! class; the algorithm is identical so the Rust port extracts it into
//! a free function (DRY).
//!
//! ## Signed base-index note
//!
//! [`decode_reverse_tree`] takes `base_idx: i64` because the LZMA
//! distance coder for slot 4 computes `base - slot - 1 = -1`. The
//! tree walk starts at `m = 1`, so the effective model index is
//! `base_idx + m = -1 + 1 = 0` — valid. The C SDK achieves the same
//! effect via pointer arithmetic (`probs + SpecPos + distance -
//! posSlot - 1 + m` where `SpecPos = -4`).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::bit_model::BitModel;
use crate::LzmaError;
use crate::RangeDecoder;

/// Decode a `num_bits`-wide symbol by walking a binary bit-tree forwards
/// (MSB first). `models[1]` is the root; child indices are `(parent << 1)
/// | bit`. The returned symbol has its MSB at position `num_bits - 1`.
///
/// # Errors
///
/// Forwards any [`LzmaError`] from the underlying range decoder.
#[inline]
pub fn decode_tree(
    range_decoder: &mut RangeDecoder<'_>,
    models: &mut [BitModel],
    num_bits: u32,
) -> Result<u32, LzmaError> {
    let mut node = 1i64;
    let mut symbol = 0u32;
    for i in (0..num_bits).rev() {
        let idx = node as usize;
        let bit = range_decoder.decode_bit(&mut models[idx])?;
        node = (node << 1) | i64::from(bit);
        symbol |= bit << i;
    }
    Ok(symbol)
}

/// Decode a `num_bits`-wide symbol by walking a binary bit-tree backwards
/// (LSB first). Used by the distance coder's reverse-tree paths. The
/// `base_idx` shifts the model array so multiple trees can share a
/// single allocation.
///
/// `base_idx` is `i64` (not `usize`) because the LZMA distance coder
/// for slot 4 passes `-1`: the tree walk adds `m` starting from `1`,
/// making the effective index `0`. See the module-level "signed
/// base-index note" for details.
///
/// # Errors
///
/// Forwards any [`LzmaError`] from the underlying range decoder.
#[inline]
pub fn decode_reverse_tree(
    range_decoder: &mut RangeDecoder<'_>,
    models: &mut [BitModel],
    base_idx: i64,
    num_bits: u32,
) -> Result<u32, LzmaError> {
    let mut node = 1i64;
    let mut symbol = 0u32;
    for i in 0..num_bits {
        let idx = usize::try_from(base_idx + node).map_err(|_| LzmaError::Corrupt {
            reason: format!(
                "reverse-tree index {base_idx}+{node} underflows: slot-4 distance with wrong base"
            ),
        })?;
        let bit = range_decoder.decode_bit(&mut models[idx])?;
        node = (node << 1) | i64::from(bit);
        symbol |= bit << i;
    }
    Ok(symbol)
}
