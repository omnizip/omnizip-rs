//! Distance-code configuration (RFC 7932 §9.4).
//!
//! Controls NPOSTFIX and NDIRECT, which together determine the
//! distance-code alphabet layout:
//!
//! ```text
//!   [0..16)          = short codes (ring buffer)
//!   [16..16+NDIRECT) = direct codes (no extra bits)
//!   [16+NDIRECT..)   = long codes (with optional postfix bits)
//! ```

/// Number of short distance codes (RFC 7932 §10.4).
pub const NUM_SHORT: u32 = 16;

/// Distance-code configuration for a metablock.
///
/// Encapsulates the NPOSTFIX and NDMOEM fields from the metablock
/// header, providing a clean API for distance encoding/decoding.
#[derive(Clone, Copy, Debug)]
pub struct DistanceConfig {
    /// NPOSTFIX field (0-3): postfix bits for long-form distance codes.
    pub npostfix: u8,
    /// NDMOEM field (0-15): determines NDIRECT = NDMOEM << NPOSTFIX.
    pub ndmoem: u8,
}

impl DistanceConfig {
    /// Create a new config with the given parameters.
    pub const fn new(npostfix: u8, ndmoem: u8) -> Self {
        Self { npostfix, ndmoem }
    }

    /// NDIRECT = NDMOEM << NPOSTFIX (RFC 7932 §9.4).
    pub const fn ndirect(&self) -> u32 {
        (self.ndmoem as u32) << self.npostfix
    }

    /// Total direct + short codes in the alphabet.
    pub const fn num_direct(&self) -> u32 {
        NUM_SHORT + self.ndirect()
    }

    /// Full distance alphabet size.
    pub fn alphabet_size(&self) -> usize {
        self.num_direct() as usize + (48usize << self.npostfix)
    }

    /// Choose NPOSTFIX/NDMOEM heuristically from the distance distribution.
    ///
    /// Fast O(N) scan: counts distances <= 15 and picks NDIRECT=12 if
    /// >=20% of distances are short (beneficial for direct codes).
    #[must_use]
    pub fn choose(commands: &[super::super::from_spec_encoder::Command]) -> Self {
        let mut short_count = 0u32;
        let mut total = 0u32;
        for cmd in commands {
            if cmd.copy_len > 0 && cmd.distance > 0 {
                total += 1;
                if cmd.distance <= 15 {
                    short_count += 1;
                }
            }
        }
        let ndmoem = if total > 0 && short_count * 5 >= total * 4 {
            12
        } else {
            0
        };
        Self::new(0, ndmoem)
    }
}
