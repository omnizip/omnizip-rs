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

    /// Choose NPOSTFIX/NDMOEM by evaluating multiple configurations and
    /// picking the one with the lowest estimated encoded size.
    ///
    /// For each candidate config, computes the distance symbol
    /// distribution and estimates the Huffman cost as Shannon entropy
    /// plus alphabet description overhead.
    pub fn choose(commands: &[super::super::from_spec_encoder::Command]) -> Self {
        let distances: Vec<u32> = commands
            .iter()
            .filter(|c| c.copy_len > 0 && c.distance > 0)
            .map(|c| c.distance)
            .collect();

        if distances.is_empty() {
            return Self::new(0, 0);
        }

        let candidates = [
            Self::new(0, 0),
            Self::new(0, 6),
            Self::new(0, 12),
            Self::new(0, 15),
            Self::new(1, 0),
            Self::new(1, 4),
            Self::new(2, 0),
        ];

        let mut best_config = Self::new(0, 0);
        let mut best_cost = u64::MAX;

        for cfg in &candidates {
            let cost = estimate_cost(cfg, &distances);
            if cost < best_cost {
                best_cost = cost;
                best_config = *cfg;
            }
        }

        best_config
    }
}

/// Estimate the encoded size (in bits) for a given config and distance
/// distribution.
///
/// Cost model:
/// - Per distance: Huffman symbol cost (Shannon entropy) + extra bits
/// - Alphabet overhead: ~3 bits per non-zero symbol for table description
fn estimate_cost(cfg: &DistanceConfig, distances: &[u32]) -> u64 {
    let alphabet = cfg.alphabet_size();
    let ndirect = cfg.ndirect() as usize;
    let num_direct = cfg.num_direct() as usize;

    let mut freq = vec![0u32; alphabet];
    let mut total_extra_bits: u64 = 0;

    for &dist in distances {
        let (sym, extra_bits) = encode_distance_symbol(dist, cfg);
        if (sym as usize) < alphabet {
            freq[sym as usize] += 1;
        }
        total_extra_bits += extra_bits as u64;
    }

    let total: u64 = distances.len() as u64;

    let mut entropy_cost: u64 = 0;
    let mut nonzero_symbols: u64 = 0;
    for &count in &freq {
        if count > 0 {
            nonzero_symbols += 1;
            let p = count as f64 / total as f64;
            let bits = (-p.log2()).ceil() as u64;
            entropy_cost += bits * count as u64;
        }
    }

    let alphabet_overhead = nonzero_symbols * 5;
    let table_overhead = (alphabet as u64 + 7) / 8;

    entropy_cost + total_extra_bits + alphabet_overhead + table_overhead
}

/// Compute the distance symbol and extra bits for a given distance.
/// Mirrors `encode_distance` in from_spec_encoder.rs but standalone.
fn encode_distance_symbol(distance: u32, cfg: &DistanceConfig) -> (u32, u32) {
    let ndirect = cfg.ndirect();

    if distance <= ndirect {
        return (NUM_SHORT + distance - 1, 0);
    }

    let d = distance - 1 - ndirect;
    let mut nbits: u32 = 1;
    while nbits < 24 {
        let limit_even = (4u32 << (nbits - 1)).saturating_sub(4) + (1u32 << nbits);
        let limit_odd = (6u32 << (nbits - 1)).saturating_sub(4) + (1u32 << nbits);
        let limit = limit_even.max(limit_odd);
        if d < limit {
            break;
        }
        nbits += 1;
    }
    let even_offset = (4u32 << (nbits - 1)).saturating_sub(4);
    let odd_offset = (6u32 << (nbits - 1)).saturating_sub(4);
    let (postfix_bit, base) = if d >= odd_offset {
        (1, odd_offset)
    } else {
        (0, even_offset)
    };
    let distval = (nbits - 1) * 2 + postfix_bit;
    let sym = cfg.num_direct() + distval;
    let extra = d - base;
    (sym, extra)
}
