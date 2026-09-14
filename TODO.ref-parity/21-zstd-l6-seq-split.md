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

## Re-measurement (2026-09-13, post v0.21.88 — tasks 22/23/25 landed)

The gate extension was re-measured after three releases that directly
cut per-partition emission cost (two-queue tables .85, histogram .86,
seqcost cache .87):

- fits L6 with splitter: 4.49s/6 user vs ref 0.97s/20 → **T = 15.4**
  (was ~31 pre-.85). Size 2,482,006 (−13.6% vs unsplit 2,877,070 —
  the banked win intact, modulo tie-noise), round-trip ok.
- Profile: `derive_splits` = 77% of the split encode (the
  trial-emission loop — `estimate_partition` still runs full
  `encode_content_parts` per candidate); final per-partition
  emissions ~23%.
- Bar to ship on I: T ≤ 3.0 (to beat the unsplit I=2.7 given
  S=0.901). Both the trial loop (incremental entropy estimates — the
  reference's shape, a full port) and the emission per-op gap would
  need ~5x. STILL BLOCKED; chain unchanged, numbers refreshed.

## Gate 2 research closure (2026-09-14): the cheap estimator does not exist

The semi-exact program: (1) pure Shannon entropy — task 27, blind
(csv2m +4.6%); (2) + real Huffman lengths (two-queue) + FSE
normalization costs (nbBits = tableLog − highbit(norm[s]), exact
rounding model) + real write_ncount header sizes + Repeat_Mode
exact-match discount — words within 6 bytes and plists +0.06% of the
exact splitter, **but csv2m still +5.5%**. The decisive dump: the
exact splitter's csv2m L19 output is **250 partitions with a rich
per-partition table-mode mix** (32× OF=RLE, 28× OF=REP+ML=REP, 20×
ML=Predefined …) — the 7KB win comes from pick_table's MEASURED
per-partition mode search (RLE 1-byte / REPEAT 0-byte / Predefined
0-byte tables), which no histogram-level model reproduces. A faithful
estimator must replicate the emitter's mode search — at which point
it IS the emitter; the only remaining lever is write-elision via the
.87 cached-cost machinery (~1.4×, vs the gate's 5× bar).

**Task 21's reopen condition is now precise: it needs ~5× cheaper
exact emission, or a fundamentally different splitting algorithm —
not a better estimator. Experimental code preserved in the session
scratch (/tmp/zwork/se_v2.rs); main reverted byte-identical.**
