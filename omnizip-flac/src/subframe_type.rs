//! Shared FLAC subframe type codes (per FLAC spec §6).
//!
//! Defined once here so the encoder and decoder agree on wire format.
//! Bits 1..6 of the subframe header byte carry the type code:
//!
//! ```text
//! 0 | type[5:0] | wasted_flag
//! ```
//!
//! - `CONSTANT` (0): all samples equal
//! - `VERBATIM` (1): raw samples
//! - `FIXED_BASE + order` (8..=12): polynomial prediction + Rice residual
//! - `LPC_BASE + (order - 1)` (32..=63): LPC + Rice residual

#![forbid(unsafe_code)]

/// Subframe type code: CONSTANT.
pub const TYPE_CONSTANT: u8 = 0b000000;
/// Subframe type code: VERBATIM.
pub const TYPE_VERBATIM: u8 = 0b000001;
/// Base for FIXED subframes; add the predictor order (0..=4).
pub const TYPE_FIXED_BASE: u8 = 0b001000;
/// Base for LPC subframes; add (order - 1) where order is 1..=32.
pub const TYPE_LPC_BASE: u8 = 0b100000;

/// Maximum FIXED predictor order (per FLAC spec).
pub const MAX_FIXED_ORDER: u8 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_codes_match_spec() {
        assert_eq!(TYPE_CONSTANT, 0);
        assert_eq!(TYPE_VERBATIM, 1);
    }

    #[test]
    fn fixed_base_is_eight_plus_order() {
        assert_eq!(TYPE_FIXED_BASE, 8);
        assert_eq!(TYPE_FIXED_BASE + 0, 8);
        assert_eq!(TYPE_FIXED_BASE + MAX_FIXED_ORDER, 12);
    }

    #[test]
    fn lpc_base_is_32_plus_order_minus_one() {
        assert_eq!(TYPE_LPC_BASE, 32);
        // Order 1 → 32, order 32 → 63
        assert_eq!(TYPE_LPC_BASE + (1 - 1), 32);
        assert_eq!(TYPE_LPC_BASE + (32 - 1), 63);
    }
}
