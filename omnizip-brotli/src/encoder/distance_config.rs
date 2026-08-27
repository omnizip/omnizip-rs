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
/// Encapsulates the NPOSTFIX and NDIRECT_CODE fields from the metablock
/// header, providing a clean API for distance encoding/decoding.
#[derive(Clone, Copy, Debug)]
pub struct DistanceConfig {
    /// NPOSTFIX field (0-3): postfix bits for long-form distance codes.
    pub npostfix: u8,
    /// NDIRECT_CODE field (0-15): determines NDIRECT = NDIRECT_CODE << NPOSTFIX.
    pub ndirect_code: u8,
}

impl DistanceConfig {
    /// Create a new config with the given parameters.
    pub const fn new(npostfix: u8, ndirect_code: u8) -> Self {
        Self {
            npostfix,
            ndirect_code,
        }
    }

    /// NDIRECT = NDIRECT_CODE << NPOSTFIX (RFC 7932 §9.4).
    pub const fn ndirect(&self) -> u32 {
        (self.ndirect_code as u32) << self.npostfix
    }

    /// Total direct + short codes in the alphabet.
    pub const fn num_direct(&self) -> u32 {
        NUM_SHORT + self.ndirect()
    }

    /// Full distance alphabet size.
    pub fn alphabet_size(&self) -> usize {
        self.num_direct() as usize + (48usize << self.npostfix)
    }

    /// Choose NPOSTFIX/NDIRECT_CODE heuristically from the distance distribution.
    ///
    /// Fast O(N) scan: counts distances <= 15 and picks NDIRECT=12 if
    /// >=20% of distances are short (beneficial for direct codes).
    #[must_use]
    pub fn choose(commands: &[super::super::from_spec_encoder::Command], quality: i32) -> Self {
        // BROTLI_NPOSTFIX forces a specific config (measurement).
        if let Ok(np) = std::env::var("BROTLI_NPOSTFIX") {
            let ndc = std::env::var("BROTLI_NDIRECT_CODE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3);
            return Self::new(np.parse().unwrap_or(0), ndc);
        }
        // Upstream runs the distance-parameter cost search only at q10+
        // (BrotliBuildMetaBlock vs the greedy metablock path); q4-9 keep
        // the fast heuristic.
        if quality >= 10 && !std::env::var("BROTLI_NO_DSEARCH").is_ok() {
            if let Some(cfg) = Self::search(commands) {
                return cfg;
            }
        }

        // Entropy-based selection: estimate the Huffman cost of the
        // distance symbol stream under each candidate and pick the
        // cheapest. NPOSTFIX=1 halves the effective alphabet for
        // pair-clustered distances (the reference's H5/H6 hashers
        // naturally produce these).
        //
        // The estimate is a pure function of the per-candidate symbol
        // histogram (order-independent), so all five histograms are
        // built in ONE pass over the commands — the previous shape
        // allocated one Vec per candidate and walked the distance
        // stream five times (~4% of q2 encode time).
        let candidates = [
            Self::new(0, 0),
            Self::new(0, 12),
            Self::new(1, 0),
            Self::new(1, 3),
            Self::new(2, 0),
        ];
        let mut freqs = [[0u32; 256]; 5];
        let mut any = false;
        for c in commands.iter() {
            if c.copy_len > 0 && c.distance > 0 {
                any = true;
                for (k, cfg) in candidates.iter().enumerate() {
                    let alphabet = cfg.alphabet_size();
                    let idx = (symbol_for_cost(c.distance, cfg) as usize).min(alphabet - 1);
                    freqs[k][idx] += 1;
                }
            }
        }
        if !any {
            return Self::new(0, 0);
        }
        let mut best = candidates[0];
        let mut best_cost = u64::MAX;
        for (k, cfg) in candidates.iter().enumerate() {
            let alphabet = cfg.alphabet_size();
            let total: u32 = freqs[k][..alphabet].iter().sum();
            if total == 0 {
                continue;
            }
            let mut bits = 0u64;
            for &f in freqs[k][..alphabet].iter() {
                if f > 0 {
                    let p = f as f64 / total as f64;
                    bits += (f as u64) * (-p.log2()).ceil() as u64;
                }
            }
            let cost = bits;
            if cost < best_cost {
                best_cost = cost;
                best = *cfg;
            }
        }
        best
    }
}

impl DistanceConfig {
    /// Upstream `BrotliBuildMetaBlock`'s ComputeDistanceCost search: for
    /// each (npostfix, ndirect) trial, re-encode every explicit distance
    /// under the candidate params and cost the symbol histogram with the
    /// reference PopulationCost (which models the tree header) plus the
    /// distance extra bits. The ndirect walk mirrors upstream exactly,
    /// including its between-npostfix halving. Returns None when some
    /// distance exceeds a trial's max_distance (upstream skips such
    /// configs the same way).
    fn search(commands: &[super::super::from_spec_encoder::Command]) -> Option<Self> {
        let mut best = Self::new(0, 0);
        let mut best_cost = f64::INFINITY;
        let mut ndirect_msb: u32 = 0;
        for npostfix in 0..=3u8 {
            while ndirect_msb < 16 {
                let cfg = Self::new(npostfix, ndirect_msb as u8);
                let mut hist = vec![0u32; cfg.alphabet_size()];
                let mut extra_bits = 0f64;
                let mut ok = true;
                for c in commands {
                    if c.copy_len == 0 || c.distance == 0 {
                        continue;
                    }
                    if c.distance <= NUM_SHORT + cfg.ndirect() {
                        // Short/direct codes carry no extra bits and no
                        // re-encoding risk.
                        hist[(c.distance + NUM_SHORT - 1) as usize] += 1;
                        continue;
                    }
                    let (sym, _extra, nbits) = prefix_encode_distance(c.distance, &cfg);
                    let idx = sym as usize;
                    if idx >= hist.len() {
                        ok = false;
                        break;
                    }
                    hist[idx] += 1;
                    extra_bits += f64::from(nbits);
                }
                if !ok {
                    break;
                }
                let cost = crate::encoder::block_splitter::population_cost(&hist) + extra_bits;
                if cost > best_cost {
                    break;
                }
                best_cost = cost;
                best = cfg;
                ndirect_msb += 1;
            }
            if ndirect_msb > 0 {
                ndirect_msb -= 1;
            }
            ndirect_msb /= 2;
        }
        Some(best)
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
    // Distances near MAX_BACKWARD (~2^24) overflow the bucket shift in
    // debug builds (checked shift panics); clamp to the alphabet size.
    let safe_nbits = nbits.min(24);
    let offset = (((2 + (distval & 1)) << safe_nbits) - 4) & (cfg.alphabet_size() - 1) as u32;
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

/// Upstream `PrefixEncodeCopyDistance` (prefix.h), driven from the raw
/// LZ77 distance: the caller's distance code is `distance + 15`.
/// Returns (symbol, extra_value, extra_bit_count).
pub fn prefix_encode_distance(distance: u32, cfg: &DistanceConfig) -> (u32, u32, u32) {
    let num_direct = cfg.ndirect();
    let distance_code = distance + NUM_SHORT - 1;
    if distance_code < NUM_SHORT + num_direct {
        return (distance_code, 0, 0);
    }
    let postfix_bits = u32::from(cfg.npostfix);
    let dist = (1u64 << (postfix_bits + 2)) + u64::from(distance_code - NUM_SHORT - num_direct);
    let bucket = 63 - dist.leading_zeros() - 1; // Log2FloorNonZero(dist) - 1
    let postfix_mask = (1u64 << postfix_bits) - 1;
    let postfix = (dist & postfix_mask) as u32;
    let prefix = u32::try_from((dist >> bucket) & 1).unwrap_or(0);
    let offset = ((2 + u64::from(prefix)) << bucket) as u64;
    let nbits = bucket - postfix_bits;
    let sym = NUM_SHORT + num_direct + (((2 * (nbits - 1) + prefix) << postfix_bits) + postfix);
    let extra = ((dist - offset) >> postfix_bits) as u32;
    (sym, extra, nbits)
}
