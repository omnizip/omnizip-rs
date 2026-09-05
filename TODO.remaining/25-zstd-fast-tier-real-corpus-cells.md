# 25 — zstd fast-tier cells on real content (task 09 re-opened)

- **Priority:** LOW (deferred family; new evidence from task 15's corpus)
- **Status:** closed 2026-09-05 — root-caused (price-behavior class, lever identified)

Task 09 closed the fast tier at "worst L1/L2 cell 1.0103x" on the
synthetic corpus. The real-world corpus (task 15) found cells the
synthetic one hid:

- `noto-otf.bin` (OTF/CFF fonts) zstd **L1 1.059x** — the worst
  standing fast-tier cell.
- `rfc.txt` L1 1.021x, `fits4m.bin` L1 1.015x.

## Work

1. Sweep L1/L2/L3 on the font class vs reference; check whether the
   gap is matcher recall (hash/chain shape on font tables) or level
   mapping (our L1 = ref -1 exactly?).
2. Compare against arial (0.975-0.99 across levels) — what differs
   about OTF/CFF glyph data vs TTF?
3. Fix if a bounded lever exists (min-match, hash width); otherwise
   document as match-quality class with the numbers.

## Acceptance

- Font-class L1 ≤1.02x or root-caused with the measured reason.
- No regression on the 10-file + real corpus sweep.


## Resolution (2026-09-05) — root-caused; fix lever documented

Measured shape (noto-otf.bin, 160 784 B): the deficit is FAST-TIER-ONLY
and identical at both fast levels — L1 106,906 vs ref 100,948
(1.0590x), L2 100,467 vs 94,881 (1.0589x) — while L3 (0.977), L6
(0.972), and L12 (0.988) all BEAT the reference. Not a recall problem.

Sequence-level decomposition at L1 (both streams through our decoder
with ZSTD_SEQ_DUMP):

| | sequences | match bytes | avg match len |
|---|---|---|---|
| ours | 4 714 | 107 126 | 22.7 |
| ref | 6 281 | 97 560 | 15.5 |

Ours finds FEWER, LONGER matches covering MORE input — and still
loses, because the per-sequence entropy (offset codes + FSE) of the
sparser-but-richer parse costs more than the reference's dense, cheap
row-matcher parse. Same price-vs-emission family as the documented
q11 cells, on the fast tier.

**Lever (for a future session, with the full corpus sweep + regression
gate the class requires):** the fast tier takes every match ≥ a
hardcoded floor — `params.min_match.max(5)` at encoder/block.rs:448
and :668. A content-aware accept bar (skip a 5-byte match when the
literals it replaces are cheap, like zstd's fast-strategy quality
check) should move toward the reference's parse shape; EVERY L1/L2
cell must be swept before changing the default (current corpus: L1
0.99-1.00 everywhere except fonts 1.059 and fits 1.015).
