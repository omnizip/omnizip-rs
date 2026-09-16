# Task 47 — the session-wide validation gate

Status: done (2026-09-16)

The 2026-09-15/16 session shipped the campaign's largest output-
changing set (v0.21.90-.97: f32 cost model, hierarchical clustering,
treecap margins, the q2-9 fragment band, the 2MiB contest gate) plus
the SIMD disconfirmation (task 46). Final comprehensive gates, run in
one pass against the shipped tree:

- brotli q0-11 x 11 corpus files, decode via the reference CLI:
  **132/132 byte-exact round-trips**.
- zstd L1/2/3/4/5/6/9/12/16/19/22 x 11 files, decode via the
  reference CLI: **121/121 byte-exact round-trips**.
- omnizip-brotli 104+1, omnizip-zstd 191 lib tests, the refreshed
  regression baseline, fmt, and the CI suites (linux/macOS/windows +
  differential + downstream LimniFS canary) green on every merge this
  session (PRs #599-#618).

## Terminal board (v39, unchanged)

21/77 cells above I=1.3 (median 0.80), every one a confirmed faithful-
tier floor — now by five independent methods (task 30 shapes, task 40
restructures, task 43 fidelity audits, flat LTO, task 46's real-SIMD
disconfirmation). The remaining gap is the measured cost of
#![forbid(unsafe_code)] scalar Rust against hand-tuned C on sequential
hot loops. Reopen conditions: an algorithmic breakthrough, or the
owner explicitly choosing an I-gaming trade (raw-block zstd) that
ships non-compression — not taken.


## Addendum (2026-09-16): the 1.2 bar and the hash-closure test

The target tightened 1.3 -> 1.2: **23 cells** now above it (the 21
plus rustsrc zstd L19 1.30 and plists brotli q1 1.30). Task 46's
bounds-check diagnosis got its direct test: the fast matcher's
`hash_at` (variable-length copy_from_slice + runtime `match mm`
dispatch per position — C does one unaligned read + multiply) was
rewritten exact-preserving (masked fixed 8-byte read, guarded tail).
Result: timing FLAT (words L1 3.69->3.73s, fits 2.67->2.66s, load 30)
— LLVM had already inlined the closure well. REVERTED. Sixth
confirmation: the sequential hot loops have no removable per-position
overhead left in safe scalar Rust. The 1.2 bar carries the same
reopen conditions as 1.3 with more distance to cover; the deep cells
need -37..-67%, beyond every measured model.
