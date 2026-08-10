//! Literal context computation (RFC 7932 §10.1).
//!
//! Each literal byte is encoded using a Huffman tree selected by a
//! context ID derived from the previous 1-2 bytes. The context
//! function depends on `CONTEXT_MODE`.

use crate::static_codes::K_UTF8_CONTEXT_LOOKUP;
use omnizip_codecs::ContentType;

/// Check if input looks like text (printable ASCII + whitespace).
/// Used to select UTF8 context mode over LSB6 for better ratio.
///
/// Thin wrapper around `ContentType::detect().is_text_like()`
/// (TODO 256). Kept as a Brotli-local function for the existing
/// call sites that read it as a boolean.
#[must_use]
pub fn is_text_like(input: &[u8]) -> bool {
    ContentType::detect(input).is_text_like()
}

/// Compute a literal context ID (RFC 7932 §10.1) for the given mode.
///
/// - `mode == 0` (LSB6): `p1 & 0x3F` (6-bit context from previous byte)
/// - `mode == 2` (UTF8): lookup-table-based context separating UTF-8
///   character classes
///
/// MSB6 (1) and Signed (3) are not used by the encoder but documented
/// for completeness.
#[must_use]
pub fn compute_context_id(p1: u8, p2: u8, mode: u32) -> u8 {
    match mode {
        0 => p1 & 0x3F, // LSB6
        2 => K_UTF8_CONTEXT_LOOKUP[p1 as usize] | K_UTF8_CONTEXT_LOOKUP[(p2 as usize) | 256],
        _ => p1 & 0x3F, // fallback to LSB6
    }
}

/// Cluster 64 literal contexts into `num_trees` groups based on
/// byte-frequency similarity. Returns a 64-entry context map where
/// each entry is the tree index (0..num_trees).
///
/// Uses greedy agglomerative merging with L1 distance on raw
/// histograms. Integer-only arithmetic for full determinism.
#[must_use]
pub fn cluster_contexts(histograms: &[[u32; 256]], num_trees: usize) -> Vec<u8> {
    let n = histograms.len();
    if n <= num_trees {
        return (0..n as u8).collect();
    }

    let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    let mut merged_hists: Vec<[u32; 256]> = histograms.to_vec();

    while clusters.len() > num_trees {
        let mut best_dist = u64::MAX;
        let mut best_i = 0;
        let mut best_j = 1;
        for i in 0..clusters.len() {
            for j in (i + 1)..clusters.len() {
                let dist = l1_distance(&merged_hists[i], &merged_hists[j]);
                if dist < best_dist {
                    best_dist = dist;
                    best_i = i;
                    best_j = j;
                }
            }
        }

        for k in 0..256 {
            merged_hists[best_i][k] =
                merged_hists[best_i][k].saturating_add(merged_hists[best_j][k]);
        }
        let moved = clusters.remove(best_j);
        clusters[best_i].extend(moved);
        merged_hists.remove(best_j);
    }

    let mut ctx_map = vec![0u8; n];
    for (tree_idx, cluster) in clusters.iter().enumerate() {
        for &ctx in cluster {
            ctx_map[ctx] = tree_idx as u8;
        }
    }
    ctx_map
}

/// L1 (Manhattan) distance between two byte-frequency histograms.
fn l1_distance(a: &[u32; 256], b: &[u32; 256]) -> u64 {
    let mut dist: u64 = 0;
    for i in 0..256 {
        if a[i] > b[i] {
            dist += (a[i] - b[i]) as u64;
        } else {
            dist += (b[i] - a[i]) as u64;
        }
    }
    dist
}

/// Collect per-context byte frequency histograms from the input.
/// Walks the input computing context IDs and accumulating byte counts.
#[must_use]
pub fn collect_context_histograms(input: &[u8], context_mode: u32) -> Vec<[u32; 256]> {
    let mut histograms = vec![[0u32; 256]; 64];
    let mut p1: u8 = 0;
    let mut p2: u8 = 0;
    for &b in input {
        let ctx = compute_context_id(p1, p2, context_mode) as usize;
        if ctx < 64 {
            histograms[ctx][b as usize] += 1;
        }
        p2 = p1;
        p1 = b;
    }
    histograms
}
