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

    // The greedy merge is O(n^3): at block-split scale (thousands of
    // (block, context) buckets) it dominates encode time. Reduce first:
    // drop empty buckets, then cluster per contiguous group of 64
    // (block-local contexts are the natural similarity units), then
    // greedy-merge the per-group centroids.
    let nonzero: Vec<usize> = (0..n)
        .filter(|&i| histograms[i].iter().any(|&x| x > 0))
        .collect();

    let group = 64usize;
    let mut local_maps: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut centroids: Vec<[u32; 256]> = Vec::new();
    for chunk in nonzero.chunks(group) {
        let hists: Vec<[u32; 256]> = chunk.iter().map(|&i| histograms[i]).collect();
        // Up to 8 centroids per group keeps the second phase small.
        let k = 8.min(hists.len());
        let local = cluster_contexts_greedy(&hists, k);
        let mut sums: Vec<[u32; 256]> = vec![[0u32; 256]; k];
        for (gi, &orig_i) in chunk.iter().enumerate() {
            let t = local[gi] as usize;
            for (b, &v) in histograms[orig_i].iter().enumerate() {
                sums[t][b] = sums[t][b].saturating_add(v);
            }
        }
        centroids.extend(sums);
        local_maps.push((chunk.len(), local));
    }

    let top = num_trees.min(centroids.len()).max(1);
    let centroid_map = cluster_contexts_greedy(&centroids, top);

    let mut ctx_map = vec![0u8; n];
    let mut nonzero_cursor = 0usize;
    let mut centroid_cursor = 0usize;
    for (len, local) in &local_maps {
        let k = centroids_per_chunk(*len);
        for (gi, &l) in local.iter().enumerate() {
            let centroid_idx = centroid_cursor + l as usize;
            let orig_i = nonzero[nonzero_cursor + gi];
            ctx_map[orig_i] = centroid_map[centroid_idx];
        }
        nonzero_cursor += len;
        centroid_cursor += k;
    }
    ctx_map
}

fn centroids_per_chunk(chunk_len: usize) -> usize {
    8.min(chunk_len).max(1)
}

/// Greedy agglomerative merge by L1 distance (exact, O(n^3)).
/// Only called on reduced inputs (<= a few hundred histograms).
fn cluster_contexts_greedy(histograms: &[[u32; 256]], num_trees: usize) -> Vec<u8> {
    let n = histograms.len();
    if n <= num_trees {
        return (0..n as u8).collect();
    }

    let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    let mut merged_hists: Vec<[u32; 256]> = histograms.to_vec();

    // Distance matrix cached across merges: only pairs involving the
    // merged cluster change, so each iteration recomputes one row
    // (O(n) L1s) instead of rescanning all pairs (O(n^2)). The
    // selection order and tie-breaking match the full rescan exactly —
    // only pairs whose value is unchanged are read from cache.
    let mut dist: Vec<Vec<u64>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| {
                    if i < j {
                        l1_distance(&merged_hists[i], &merged_hists[j])
                    } else {
                        0
                    }
                })
                .collect()
        })
        .collect();

    while clusters.len() > num_trees {
        let m = clusters.len();
        let mut best_dist = u64::MAX;
        let mut best_i = 0;
        let mut best_j = 1;
        for i in 0..m {
            for j in (i + 1)..m {
                let d = dist[i][j];
                if d < best_dist {
                    best_dist = d;
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
        dist.remove(best_j);
        for row in dist.iter_mut() {
            row.remove(best_j);
        }
        // Recompute the merged row (and its symmetric entries).
        for j in 0..dist.len() {
            if best_i != j {
                let d = if best_i < j {
                    l1_distance(&merged_hists[best_i], &merged_hists[j])
                } else {
                    l1_distance(&merged_hists[j], &merged_hists[best_i])
                };
                let (lo, hi) = (best_i.min(j), best_i.max(j));
                dist[lo][hi] = d;
            }
        }
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

/// Data-driven context→tree assignment that isolates LOW-DIVERSITY
/// contexts into dedicated trees (a pure context = single-symbol tree
/// = ZERO bits per literal; a 2-symbol context = 1 bit). The remaining
/// high-diversity contexts are clustered into shared trees.
///
/// This mirrors what the reference encoder's ContextBlockSplitter
/// achieves implicitly: on regular data many (block, context) buckets
/// hold exactly one byte value, and giving each its own tree removes
/// those literals from the bitstream entirely.
pub fn assign_context_trees(
    histograms: &[[u32; 256]],
    max_shared_trees: usize,
) -> (Vec<u8>, usize) {
    let n = histograms.len();
    let mut assignment = vec![0u8; n];
    let mut next_tree: usize = 0;
    let mut shared: Vec<usize> = Vec::new();
    for (i, h) in histograms.iter().enumerate() {
        let total: u64 = h.iter().map(|&x| u64::from(x)).sum();
        let distinct = h.iter().filter(|&&x| x > 0).count();
        // Dedicated tree when the per-literal saving clearly beats the
        // ~12-30-bit tree header: pure contexts always win; 2-3 symbol
        // contexts win when common enough.
        let dedicated = distinct == 1 && total >= 8;
        if dedicated {
            assignment[i] = next_tree as u8;
            next_tree += 1;
        } else {
            shared.push(i);
        }
    }
    let ntrees = if shared.is_empty() {
        next_tree.max(1)
    } else {
        let shared_hists: Vec<[u32; 256]> = shared.iter().map(|&i| histograms[i]).collect();
        let k = max_shared_trees.min(shared.len()).max(1);
        let cmap = cluster_contexts(&shared_hists, k);
        for (slot, &i) in shared.iter().enumerate() {
            assignment[i] = (next_tree + cmap[slot] as usize) as u8;
        }
        next_tree + k
    };
    // Tree ids must fit the cmap's u8 entries.
    // Tree ids must fit the cmap's u8 entries, with margin below the
    // 256-symbol cmap-table boundary (a full 256-tree table hits wire
    // edge cases in the complex-form reader).
    let cap = std::env::var("BROTLI_TREE_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(240);
    let ntrees = ntrees.min(cap);
    for a in assignment.iter_mut() {
        *a = (*a).min(ntrees as u8 - 1);
    }
    (assignment, ntrees)
}
