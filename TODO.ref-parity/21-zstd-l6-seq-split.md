# Task 21 — zstd L5-L12: post-parse sequence splitting (diagnosed, blocked on cost)

Status: blocked (2026-09-12) — mechanism fully measured; extension reverted
Parent chain: task 20 (same decomposition method); depends on the
per-section Huffman cost fix (task 19's residual / `HUF_setMaxHeight` port)

## The finding

fits zstd L6's S=1.044 decomposed decoder-side (`ZSTD_SEC_DUMP`, now with
`n_seq`):

| side | blocks | lit_regen | n_seq | bits/seq | seq wire |
|---|---|---|---|---|---|
| ref 1.5.7 | 141 (mostly 24 KiB) | 2,742,566 | 280,610 | **12.40** | 434,898 |
| ours | 33 (127 KiB) | 2,294,628 | 364,264 | **16.32** | 743,101 |

We find 448K MORE matched bytes (fewer literals — our lazy parse is
stronger) yet lose by +121 KB entirely on sequence coding: one FSE table
set per heterogeneous 127 KiB block vs ref's per-24 KiB partitions.
Ref's small blocks come from the **post-parse sequence-stream splitter**,
which 1.5.7 runs from the greedy tier up (empirical map: fits L5/L6 → 141
blocks; words L6 → 40; plists L6 → 26; csv2m/sqlite L6 → unsplit — its own
entropy test gates content). The `-B131072` probe proves the split is
post-parse (128 KiB input blocks still yield 110 output blocks). Note:
our vendored reference checkout is 1.6.0-dev whose library gate reads
`strategy >= btopt && wlog >= 17` — the benchmark binary (homebrew 1.5.7)
evidently differs; version drift to keep in mind.

## The experiment

Our shielded post-parse splitter (kept-only-when-smaller) exists but was
gated to btopt+. Extending the gate to Greedy/Lazy/Lazy2:

- Sizes (all round-trip ok): fits L6 2,877,075 → **2,480,667 (−13.8%,
  S 1.044 → 0.901)**; csv2m L6 −5.2% (S 0.922); plists L6 −4.8% (S 0.980);
  noto L6 −3.2% (S 0.968); sqlite L6 −0.9%; L5/L9/L12 improve similarly
  (untracked columns); install/rfc unchanged.
- Time: fits L6 T 3.1 → **~31** (interleaved 3×20, load 19-39 — direction
  unambiguous). I would go 3.2 → ~28.

## Why it is blocked

`sample` profile: ~75% of encode sits in `derive_splits` → 
`estimate_partition` → `encode_content_parts` → `build_weights` →
`package_merge` — **every split-candidate evaluation runs a full literal
Huffman build** (package-merge: 11 levels × sort of ~510 coins). The
reference derives split points from incremental entropy estimates
(running histograms) and never trial-encodes. Even with a perfect derive,
the final per-partition emissions (~130 partitions × package-merge)
keep T ≈ 8 on fits — still unshippable.

## Dependency chain to reopen

1. Port the reference's literal table build (`HUF_setMaxHeight`-style
   O(m·L) rebalancing) or otherwise cut package-merge cost — this also
   unblocks task 19's residual and makes task 20's half-split cheaper.
2. Rewrite `estimate_partition` to incremental entropy accounting
   (running literal + LL/OF/ML histograms), no Huffman builds.
3. Re-extend the gate; sweep L5/L6/L9/L12 corpus-wide; fresh quiet-box T.

The −13.8% fits L6 size is banked knowledge; the gate revert restores
byte-identical v0.21.84 outputs (verified: fits L6 2,877,075, fits L1
3,576,456, plists L6 140,659).
