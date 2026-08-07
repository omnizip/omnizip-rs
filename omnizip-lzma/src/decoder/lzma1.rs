//! LZMA1 packet decoder — the core decode engine shared by every LZMA
//! container format (`.lzma`, `.xz`, `.lz`, LZMA2 chunks).
//!
//! Ported from the decode side of
//! `omnizip/lib/omnizip/algorithms/lzma/xz_utils_decoder.rb` (1,311 LOC).
//! This file implements the single-stream subset: one input slice, one
//! growing output buffer, no LZMA2 multi-chunk state preservation.
//!
//! ## What's simplified vs. the Ruby
//!
//! - **No circular buffer.** The Ruby pre-allocates `dict_size + 576`
//!   bytes and uses `dict_index(pos)` to map a monotonic position into
//!   the ring. We grow a `Vec<u8>` instead — simpler, same result for
//!   single-stream use, and we can switch to a ring later if memory
//!   becomes a concern.
//! - **No LZMA2 multi-chunk.** `prepare_state_reset`, `set_input`,
//!   `add_to_dictionary`, `compact_buffer` are absent; the LZMA2 chunk
//!   manager (future `decoder/lzma2.rs`) will drive `Lzma1Decoder`
//!   per-chunk to recover multi-chunk behaviour.
//! - **No `validate_size` strictness.** We trust `uncompressed_size`
//!   when given; the Ruby's `check_rc_finished` / EOPM-after-data checks
//!   land with the strict-mode port.
//!
//! ## Algorithm (matches XZ Utils `lzma_decoder.c`)
//!
//! ```text
//! loop:
//!   pos_state = output.len() & pb_mask
//!   if decode_bit(is_match[state*pos_states + pos_state]) == 0:
//!     decode_literal()
//!   else:
//!     if decode_bit(is_rep[state]) == 0:
//!       decode_regular_match(pos_state)   # may be EOS marker
//!     else:
//!       decode_rep_match(pos_state)
//!   stop when output.len() == uncompressed_size or EOS seen
//! ```

#![forbid(unsafe_code)]

use crate::bit_model::BitModel;
use crate::coder::{DistanceDecoder, LengthDecoder};
use crate::constants::{MATCH_LEN_MAX, MATCH_LEN_MIN, NUM_LEN_TO_POS_STATES, NUM_STATES};
use crate::range_coder::RangeDecoder;
use crate::state::LzmaState;
use crate::LzmaError;

/// Sentinel distance value indicating the LZMA end-of-payload marker
/// (EOPM). The encoder emits this when the uncompressed size is unknown.
const EOPM_DISTANCE: u32 = 0xFFFF_FFFF;

/// LZMA1 decoder — single stream. Holds all mutable state across the
/// decode: probability models, the 12-state machine, the four rep
/// distances, and the growing output buffer.
#[derive(Debug)]
pub struct Lzma1Decoder {
    lc: u32,
    lp: u32,
    pb: u32,
    pb_mask: u32,
    pb_shift: usize,
    literal_mask: u32,

    state: LzmaState,
    rep0: u32,
    rep1: u32,
    rep2: u32,
    rep3: u32,

    is_match: Vec<BitModel>,
    is_rep: Vec<BitModel>,
    is_rep0: Vec<BitModel>,
    is_rep1: Vec<BitModel>,
    is_rep2: Vec<BitModel>,
    is_rep0_long: Vec<BitModel>,

    literal_models: Vec<BitModel>,

    length_coder: LengthDecoder,
    rep_length_coder: LengthDecoder,
    distance_coder: DistanceDecoder,
}

/// Per-decode configuration. Owned by [`Lzma1Decoder::decode`] so the
/// engine itself can be reused across streams with different parameters.
#[derive(Clone, Copy, Debug)]
struct DecodeConfig {
    uncompressed_size: Option<u64>,
    allow_eopm: bool,
    /// Output buffer length at the start of this decode call. The size
    /// limit check uses `(output.len() - start_output_len)` so that
    /// LZMA2 continuation chunks measure their per-chunk output, not
    /// the cumulative total across all chunks.
    start_output_len: usize,
}

impl Lzma1Decoder {
    /// Construct a decoder for the given LZMA parameters. The Ruby
    /// validates `lc ∈ [0, 8]`, `lp ∈ [0, 4]`, `pb ∈ [0, 4]`; the Rust
    /// port asserts the same ranges.
    ///
    /// `dict_size` is currently unused — the decoder uses a growing
    /// `Vec<u8>` rather than the Ruby's pre-allocated ring. It's kept
    /// for API compatibility with the decoder interface
    /// continuation that switches back to a circular buffer.
    ///
    /// # Panics
    ///
    /// Panics if any parameter is out of range.
    #[must_use]
    pub fn new(lc: u32, lp: u32, pb: u32, _dict_size: u32) -> Self {
        assert!(lc <= 8, "lc must be 0..=8, got {lc}");
        assert!(lp <= 4, "lp must be 0..=4, got {lp}");
        assert!(pb <= 4, "pb must be 0..=4, got {pb}");
        assert!(
            lc + lp <= 4,
            "lc + lp must be ≤ 4 (got {lc} + {lp} = {})",
            lc + lp,
        );

        let pos_states = 1usize << pb;
        let pb_mask = pos_states as u32 - 1;
        let pb_shift = pos_states;
        // XZ Utils: literal_mask = (0x100 << lp) - (0x100 >> lc)
        let literal_mask = (0x100u32 << lp).wrapping_sub(0x100u32 >> lc);

        Self {
            lc,
            lp,
            pb,
            pb_mask,
            pb_shift,
            literal_mask,
            state: LzmaState::initial(),
            rep0: 0,
            rep1: 0,
            rep2: 0,
            rep3: 0,
            is_match: vec![BitModel::new(); NUM_STATES * pos_states],
            is_rep: vec![BitModel::new(); NUM_STATES],
            is_rep0: vec![BitModel::new(); NUM_STATES],
            is_rep1: vec![BitModel::new(); NUM_STATES],
            is_rep2: vec![BitModel::new(); NUM_STATES],
            is_rep0_long: vec![BitModel::new(); NUM_STATES * pos_states],
            literal_models: {
                // Size the literal table to fit the maximum model index
                // the unmatched + matched sub-coders can produce:
                //   base_offset = 3 * (max_context_value << lc)
                //   max_index = base_offset + 0x300  (matched mode)
                let max_context_value = literal_mask;
                let max_base_offset = (max_context_value * 3) << lc;
                let max_index = max_base_offset + 0x300;
                vec![BitModel::new(); (max_index as usize) + 1]
            },
            length_coder: LengthDecoder::new(pos_states),
            rep_length_coder: LengthDecoder::new(pos_states),
            distance_coder: DistanceDecoder::new(NUM_LEN_TO_POS_STATES as usize),
        }
    }

    /// Read-only access to the LZMA `lc` parameter.
    #[must_use]
    pub const fn lc(&self) -> u32 {
        self.lc
    }

    /// Read-only access to the LZMA `lp` parameter.
    #[must_use]
    pub const fn lp(&self) -> u32 {
        self.lp
    }

    /// Read-only access to the LZMA `pb` parameter.
    #[must_use]
    pub const fn pb(&self) -> u32 {
        self.pb
    }

    /// Decode `input` as an LZMA1 stream, producing the original bytes.
    ///
    /// - `uncompressed_size = Some(n)` — stop after producing `n` bytes.
    /// - `uncompressed_size = None` — decode until the encoder's EOPM.
    /// - `allow_eopm = true` — accept EOPM even when size is known
    ///   (LZMA-Alone allows this; LZMA2 does not).
    ///
    /// # Errors
    ///
    /// Returns [`LzmaError::Corrupt`] on truncation, invalid distance,
    /// or EOPM-where-not-allowed.
    /// Reset the LZMA state machine, rep distances, and all probability
    /// models to their initial values. Output buffer is NOT touched.
    ///
    /// Used by LZMA2 for "reset state" chunks (control bits 5-6 ≥ 1)
    /// where probability models must be reinitialised but the output
    /// buffer persists.
    pub fn reset_state(&mut self) {
        self.state = LzmaState::initial();
        self.rep0 = 0;
        self.rep1 = 0;
        self.rep2 = 0;
        self.rep3 = 0;
        self.reset_all_models();
    }

    /// Decode a chunk **without resetting state**. Appends to `output`.
    ///
    /// This is the LZMA2 continuation path: the LZMA state machine,
    /// probability models, and rep distances carry over from the
    /// previous chunk. Only the range decoder is created fresh (every
    /// LZMA2 chunk has its own range-coder initialisation).
    ///
    /// # Errors
    ///
    /// Returns [`LzmaError::Corrupt`] on truncation, invalid distance,
    /// or any decoder-side corruption.
    pub fn decode_continuation(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
        uncompressed_size: u64,
    ) -> Result<(), LzmaError> {
        let cfg = DecodeConfig {
            uncompressed_size: Some(uncompressed_size),
            allow_eopm: false,
            start_output_len: output.len(),
        };
        let mut range_decoder = RangeDecoder::new(input)?;
        self.run_decode_loop(&mut range_decoder, output, cfg)
    }

    /// Decode `input` as an LZMA1 stream, producing the original bytes.
    ///
    /// - `uncompressed_size = Some(n)` — stop after producing `n` bytes.
    /// - `uncompressed_size = None` — decode until the encoder's EOPM.
    /// - `allow_eopm = true` — accept EOPM even when size is known
    ///   (LZMA-Alone allows this; LZMA2 does not).
    ///
    /// # Errors
    ///
    /// Returns [`LzmaError::Corrupt`] on truncation, invalid distance,
    /// or EOPM-where-not-allowed.
    pub fn decode(
        &mut self,
        input: &[u8],
        uncompressed_size: Option<u64>,
        allow_eopm: bool,
    ) -> Result<Vec<u8>, LzmaError> {
        // Special-case: empty input.
        if let Some(0) = uncompressed_size {
            return Ok(Vec::new());
        }

        let cfg = DecodeConfig {
            uncompressed_size,
            allow_eopm,
            start_output_len: 0,
        };

        // Fresh per-decode state.
        self.reset_state();

        let mut range_decoder = RangeDecoder::new(input)?;
        let mut output: Vec<u8> = Vec::new();

        self.run_decode_loop(&mut range_decoder, &mut output, cfg)?;

        // Verify final size if known.
        if let Some(target) = cfg.uncompressed_size {
            let actual = output.len() as u64;
            if actual != target {
                return Err(LzmaError::Corrupt {
                    reason: format!("decoded size mismatch: expected {target}, got {actual}"),
                });
            }
        }

        Ok(output)
    }

    /// The inner decode loop, shared by [`Self::decode`] (standalone)
    /// and [`Self::decode_continuation`] (LZMA2 chunk with persistent
    /// state).
    fn run_decode_loop(
        &mut self,
        range_decoder: &mut RangeDecoder<'_>,
        output: &mut Vec<u8>,
        cfg: DecodeConfig,
    ) -> Result<(), LzmaError> {
        loop {
            // Check size limit — per-chunk, not cumulative.
            if let Some(target) = cfg.uncompressed_size {
                let produced = output.len() - cfg.start_output_len;
                if produced as u64 >= target {
                    break;
                }
            }

            let pos_state = (output.len() as u32) & self.pb_mask;
            let model_idx = (usize::from(self.state.as_u8()) * self.pb_shift)
                + usize::try_from(pos_state).unwrap_or(0);
            let is_match = range_decoder.decode_bit(&mut self.is_match[model_idx])?;

            if is_match == 0 {
                self.decode_literal(range_decoder, output)?;
            } else {
                let eos = self.decode_match(range_decoder, output, pos_state, cfg)?;
                if eos {
                    break;
                }
            }
        }

        Ok(())
    }

    /// Reset every probability model — done at the start of each decode.
    fn reset_all_models(&mut self) {
        for m in &mut self.is_match {
            m.reset();
        }
        for m in &mut self.is_rep {
            m.reset();
        }
        for m in &mut self.is_rep0 {
            m.reset();
        }
        for m in &mut self.is_rep1 {
            m.reset();
        }
        for m in &mut self.is_rep2 {
            m.reset();
        }
        for m in &mut self.is_rep0_long {
            m.reset();
        }
        for m in &mut self.literal_models {
            m.reset();
        }
        self.length_coder.reset_models();
        self.rep_length_coder.reset_models();
        self.distance_coder.reset_models();
    }

    /// Compute the literal context value used to key the literal models.
    /// Matches XZ Utils `literal_subcoder`: `((pos << 8) + prev_byte) & literal_mask`.
    fn literal_state(&self, output: &[u8]) -> u32 {
        let prev_byte = output.last().copied().unwrap_or(0);
        let pos = output.len() as u32;
        ((pos << 8) | u32::from(prev_byte)) & self.literal_mask
    }

    /// Decode a literal byte (matched or unmatched mode depending on state).
    fn decode_literal(
        &mut self,
        range_decoder: &mut RangeDecoder<'_>,
        output: &mut Vec<u8>,
    ) -> Result<(), LzmaError> {
        let lit_state = self.literal_state(output);

        let byte: u8 = if self.state.is_match_context() && !output.is_empty() {
            // Matched mode: use the byte at distance rep0+1 back.
            let back_idx = output
                .len()
                .saturating_sub(usize::try_from(self.rep0).unwrap_or(0) + 1);
            let match_byte = output.get(back_idx).copied().unwrap_or(0);
            self.decode_matched_literal(lit_state, match_byte, range_decoder)?
        } else {
            self.decode_unmatched_literal(lit_state, range_decoder)?
        };

        self.state.on_literal();
        output.push(byte);
        Ok(())
    }

    /// Decode a literal in unmatched mode (direct 8-bit tree walk).
    /// Inline copy of the algorithm so we don't need a separate
    /// `LiteralDecoder` indirection — saves a layer of `&mut` borrow
    /// juggling with the model array.
    fn decode_unmatched_literal(
        &mut self,
        lit_state: u32,
        range_decoder: &mut RangeDecoder<'_>,
    ) -> Result<u8, LzmaError> {
        let base_offset = 3 * (lit_state << self.lc);
        let mut symbol = 1u32;
        while symbol < 0x100 {
            let idx = (base_offset + symbol) as usize;
            let bit = range_decoder.decode_bit(&mut self.literal_models[idx])?;
            symbol = (symbol << 1) | bit;
        }
        Ok((symbol - 0x100) as u8)
    }

    /// Decode a literal in matched mode (uses `match_byte` for context).
    fn decode_matched_literal(
        &mut self,
        lit_state: u32,
        match_byte: u8,
        range_decoder: &mut RangeDecoder<'_>,
    ) -> Result<u8, LzmaError> {
        let base_offset = 3 * (lit_state << self.lc);
        let mut symbol = 1u32;
        let mut match_sym = u32::from(match_byte);
        let mut offset = 0x100u32;

        loop {
            match_sym <<= 1;
            let match_bit = match_sym & offset;
            let model_idx = (base_offset + offset + match_bit + symbol) as usize;
            let bit = range_decoder.decode_bit(&mut self.literal_models[model_idx])?;

            if bit == 0 {
                offset &= !match_bit;
                symbol <<= 1;
            } else {
                offset &= match_bit;
                symbol = (symbol << 1) | 1;
            }

            let match_bit_flag = u32::from(match_bit > 0);
            if match_bit_flag != bit {
                // Switch to unmatched for remaining bits.
                while symbol < 0x100 {
                    let idx = (base_offset + symbol) as usize;
                    let b = range_decoder.decode_bit(&mut self.literal_models[idx])?;
                    symbol = (symbol << 1) | b;
                }
                break;
            }

            if symbol >= 0x100 {
                break;
            }
        }

        Ok((symbol - 0x100) as u8)
    }

    /// Decode a match packet. Returns `Ok(true)` if the EOS marker was
    /// consumed; `Ok(false)` to continue the loop.
    fn decode_match(
        &mut self,
        range_decoder: &mut RangeDecoder<'_>,
        output: &mut Vec<u8>,
        pos_state: u32,
        cfg: DecodeConfig,
    ) -> Result<bool, LzmaError> {
        let state_idx = usize::from(self.state.as_u8());
        let is_rep = range_decoder.decode_bit(&mut self.is_rep[state_idx])?;

        if is_rep == 0 {
            self.decode_regular_match(range_decoder, output, pos_state, cfg)
        } else {
            self.decode_rep_match(range_decoder, output, pos_state, cfg)?;
            Ok(false)
        }
    }

    /// Decode a regular (non-rep) match. May consume the EOPM and
    /// return `Ok(true)`.
    fn decode_regular_match(
        &mut self,
        range_decoder: &mut RangeDecoder<'_>,
        output: &mut Vec<u8>,
        pos_state: u32,
        cfg: DecodeConfig,
    ) -> Result<bool, LzmaError> {
        let length_encoded = self
            .length_coder
            .decode(range_decoder, usize::try_from(pos_state).unwrap_or(0))?;
        let mut length = length_encoded + MATCH_LEN_MIN;

        // Length-state for the distance coder (XZ Utils get_dist_state).
        let len_state = if length < NUM_LEN_TO_POS_STATES + MATCH_LEN_MIN {
            length - MATCH_LEN_MIN
        } else {
            NUM_LEN_TO_POS_STATES - 1
        };

        let distance = self
            .distance_coder
            .decode(range_decoder, len_state as usize)?;

        // EOPM detection.
        if distance == EOPM_DISTANCE {
            if !cfg.allow_eopm && cfg.uncompressed_size.is_some() {
                return Err(LzmaError::Corrupt {
                    reason: "EOPM encountered but not allowed".into(),
                });
            }
            range_decoder.normalise()?;
            if range_decoder.code() != 0 {
                return Err(LzmaError::Corrupt {
                    reason: format!(
                        "EOPM detected but range decoder not finished (code={})",
                        range_decoder.code()
                    ),
                });
            }
            return Ok(true);
        }

        // Validate distance against output size.
        let dist_us = usize::try_from(distance).map_err(|_| LzmaError::Corrupt {
            reason: format!("match distance {distance} exceeds usize"),
        })?;
        if dist_us >= output.len() {
            return Err(LzmaError::Corrupt {
                reason: format!(
                    "invalid distance {distance} (bytes_written: {})",
                    output.len()
                ),
            });
        }

        length = Self::clamp_length(length, output.len(), cfg);

        Self::copy_match(output, dist_us, length);

        self.state.on_match();
        // Rotate rep distances: rep3 ← rep2 ← rep1 ← rep0; rep0 = distance
        self.rep3 = self.rep2;
        self.rep2 = self.rep1;
        self.rep1 = self.rep0;
        self.rep0 = distance;

        Ok(false)
    }

    /// Decode a rep match (reuses one of rep0/1/2/3).
    fn decode_rep_match(
        &mut self,
        range_decoder: &mut RangeDecoder<'_>,
        output: &mut Vec<u8>,
        pos_state: u32,
        cfg: DecodeConfig,
    ) -> Result<(), LzmaError> {
        let state_idx = usize::from(self.state.as_u8());
        let is_rep0_bit = range_decoder.decode_bit(&mut self.is_rep0[state_idx])?;

        let length: u32;
        let distance: u32;

        if is_rep0_bit == 0 {
            // Rep0 path.
            let long_model_idx =
                (state_idx * self.pb_shift) + usize::try_from(pos_state).unwrap_or(0);
            let is_rep0_long = range_decoder.decode_bit(&mut self.is_rep0_long[long_model_idx])?;
            if is_rep0_long == 0 {
                // Short rep: length 1, no rotation.
                length = 1;
                self.state.on_short_rep();
            } else {
                length = self
                    .rep_length_coder
                    .decode(range_decoder, usize::try_from(pos_state).unwrap_or(0))?
                    + MATCH_LEN_MIN;
                self.state.on_rep();
            }
            distance = self.rep0;
        } else {
            let is_rep1_bit = range_decoder.decode_bit(&mut self.is_rep1[state_idx])?;
            if is_rep1_bit == 0 {
                // Use rep1: swap rep0 ↔ rep1.
                distance = self.rep1;
                self.rep1 = self.rep0;
                self.rep0 = distance;
            } else {
                let is_rep2_bit = range_decoder.decode_bit(&mut self.is_rep2[state_idx])?;
                if is_rep2_bit == 0 {
                    // Use rep2: rotate rep2 → rep0.
                    distance = self.rep2;
                    self.rep2 = self.rep1;
                    self.rep1 = self.rep0;
                    self.rep0 = distance;
                } else {
                    // Use rep3: rotate rep3 → rep0.
                    distance = self.rep3;
                    self.rep3 = self.rep2;
                    self.rep2 = self.rep1;
                    self.rep1 = self.rep0;
                    self.rep0 = distance;
                }
            }
            length = self
                .rep_length_coder
                .decode(range_decoder, usize::try_from(pos_state).unwrap_or(0))?
                + MATCH_LEN_MIN;
            self.state.on_rep();
        }

        // Validate.
        let dist_us = usize::try_from(distance).map_err(|_| LzmaError::Corrupt {
            reason: format!("rep distance {distance} exceeds usize"),
        })?;
        if dist_us >= output.len() {
            return Err(LzmaError::Corrupt {
                reason: format!(
                    "invalid rep distance {distance} (bytes_written: {})",
                    output.len()
                ),
            });
        }

        let length = Self::clamp_length(length, output.len(), cfg);
        Self::copy_match(output, dist_us, length);
        // Position-state changes already applied above.
        let _ = MATCH_LEN_MAX; // pin the import; future clamp may use it.

        Ok(())
    }

    /// Cap `length` so it doesn't overshoot the declared uncompressed size.
    fn clamp_length(length: u32, current_len: usize, cfg: DecodeConfig) -> u32 {
        if let Some(target) = cfg.uncompressed_size {
            let produced = current_len - cfg.start_output_len;
            let remaining = target.saturating_sub(u64::try_from(produced).unwrap_or(u64::MAX));
            if u64::from(length) > remaining {
                return remaining as u32;
            }
        }
        length.min(MATCH_LEN_MAX)
    }

    /// Copy `length` bytes from `distance+1` back in the output. Handles
    /// overlapping ranges (RLE-style).
    fn copy_match(output: &mut Vec<u8>, distance: usize, length: u32) {
        let len = length as usize;
        let src_start = output.len() - distance - 1;
        // Reserve up-front so push doesn't reallocate mid-copy.
        output.reserve(len);
        for i in 0..len {
            let byte = output[src_start + i];
            output.push(byte);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_validates_parameters() {
        let _ = Lzma1Decoder::new(3, 0, 2, 4096);
        assert!(std::panic::catch_unwind(|| Lzma1Decoder::new(9, 0, 0, 4096)).is_err());
        assert!(std::panic::catch_unwind(|| Lzma1Decoder::new(0, 5, 0, 4096)).is_err());
        assert!(std::panic::catch_unwind(|| Lzma1Decoder::new(0, 0, 5, 4096)).is_err());
        assert!(std::panic::catch_unwind(|| Lzma1Decoder::new(3, 2, 0, 4096)).is_err());
    }

    #[test]
    fn empty_input_with_known_zero_size_returns_empty() {
        // Range decoder still needs 5 init bytes, but the early return
        // for `uncompressed_size == 0` skips decode entirely.
        let mut d = Lzma1Decoder::new(3, 0, 2, 4096);
        let out = d.decode(&[0u8; 5], Some(0), true).expect("decode");
        assert!(out.is_empty());
    }
}
