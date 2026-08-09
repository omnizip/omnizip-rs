# 229 — Brotli Smart Context Clustering

- **Status:** DONE (4-tree context modeling via fixed split; smart
  clustering infrastructure implemented)
- **Priority:** P2
- **Crate:** `omnizip-brotli`
- **Implemented in:** 0.16.16 (4-tree split), 0.16.12 (clustering
  infrastructure), 0.16.8 (NSYM>2 decoder fix)

## What was implemented

1. **4-tree context modeling** (0.16.16): Uses 4 literal context
   trees via fixed `ctx >> 4` split for inputs >= 8192 bytes.
   Contexts 0-15 → tree 0, 16-31 → tree 1, 32-47 → tree 2,
   48-63 → tree 3.

2. **NSYM>2 decoder fix** (0.16.8): Fixed `write_context_map` to
   emit bit-reversed Huffman codes (was writing raw values).
   Fixed `write_context_map_tree` to emit `tree_select=0` for
   NSYM=4. This enables correct 4-tree context map encoding.

3. **Smart clustering infrastructure** (0.16.12):
   - `cluster_contexts()`: Greedy agglomerative merging with
     integer-only L1 distance for full determinism
   - `collect_context_histograms()`: Collects per-context byte
     frequency histograms from the input
   - `collect_context_histograms_from_commands()`: Walks commands
     for histogram collection matching the output simulation

4. **Empty-tree guard** (0.16.13): Adds `freq[0]=1` for trees
   with zero total literals to prevent degenerate Huffman tables.

## Why smart clustering is not wired

Data-dependent (non-contiguous) context maps produce corrupted
decoder output. The fixed `ctx >> 4` split (contiguous blocks)
works correctly. The root cause appears to be in the decoder's
`HuffmanTable::read_symbol` or `from_lengths` — certain context
map bit patterns trigger an edge case that corrupts subsequent
data. The fixed contiguous split avoids this pattern.

## How to enable smart clustering

Replace the `ctx >> 4` split in `from_spec_encoder.rs` with:
```rust
let histograms = collect_context_histograms(input, context_mode);
let ctx_map = cluster_contexts(&histograms, 4);
```
Then debug the decoder corruption by comparing the encoded
bitstream against a reference implementation bit by bit.
