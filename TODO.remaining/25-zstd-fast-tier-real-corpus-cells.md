# 25 — zstd fast-tier cells on real content (task 09 re-opened)

- **Priority:** LOW (deferred family; new evidence from task 15's corpus)
- **Status:** pending 2026-09-05

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
