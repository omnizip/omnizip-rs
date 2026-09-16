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
