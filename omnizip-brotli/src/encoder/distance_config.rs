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
        // BROTLI_NPOSTFIX forces a specific config (measurement).
        if let Ok(np) = std::env::var("BROTLI_NPOSTFIX") {
            let nd = std::env::var("BROTLI_NDMOEM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3);
            return Self::new(np.parse().unwrap_or(0), nd);
        }

        // Entropy-based selection: estimate the Huffman cost of the
        // distance symbol stream under each candidate and pick the
        // cheapest. NPOSTFIX=1 halves the effective alphabet for
        // pair-clustered distances (the reference's H5/H6 hashers
        // naturally produce these).
        let dists: Vec<u32> = commands
            .iter()
            .filter(|c| c.copy_len > 0 && c.distance > 0)
            .map(|c| c.distance)
            .collect();
        if dists.is_empty() {
            return Self::new(0, 0);
        }
        let candidates = [
            Self::new(0, 0),
            Self::new(0, 12),
            Self::new(1, 0),
            Self::new(1, 3),
            Self::new(2, 0),
        ];
        let mut best = candidates[0];
        let mut best_cost = u64::MAX;
        for cfg in &candidates {
            let syms: Vec<u32> = dists.iter().map(|&d| symbol_for_cost(d, cfg)).collect();
            let cost = huffman_cost_estimate(&syms, cfg.alphabet_size());
            if cost < best_cost {
                best_cost = cost;
                best = *cfg;
            }
        }
        best
    }
}

fn symbol_for_cost(d: u32, cfg: &DistanceConfig) -> u32 {
    let ndirect = cfg.ndirect();
    if d < 16 + ndirect {
        return d;
    }
    let postfix_mask = (1u32 << cfg.npostfix) - 1;
    let mut distval = d - 16 - ndirect;
    let postfix = distval & postfix_mask;
    distval >>= cfg.npostfix;
    let nbits = (distval >> 1) + 1;
    let offset = ((2 + (distval & 1)) << nbits) - 4;
    let base = 16 + ndirect;
    base + ((offset) << cfg.npostfix) + postfix
}

fn huffman_cost_estimate(syms: &[u32], alphabet: usize) -> u64 {
    let mut freq = vec![0u32; alphabet];
    for &s in syms {
        let idx = (s as usize).min(alphabet - 1);
        freq[idx] += 1;
    }
    let total: u32 = freq.iter().sum();
    if total == 0 {
        return 0;
    }
    let mut bits = 0u64;
    for &f in freq.iter() {
        if f > 0 {
            let p = f as f64 / total as f64;
            bits += (f as u64) * (-p.log2()).ceil() as u64;
        }
    }
    bits
}
