# Task 34 — brotli: kill dead contest candidates (bt at q11, iter always)

Status: done (2026-09-14, shipped v0.21.90)
Board cells: rfc brotli q11 I=4.1 (THE worst cell), sqlite q11 I=3.0, and
the whole dense-text q10/q11 column

## Finding

The full q10/11 contest still ran two candidates that **never win** on
the 11-file corpus:

1. **iter** (in-house q5-tier parse, re-emitted at q5): never the
   contest min() on any file at q10 or q11. On rfc q10 it is strictly
   *worse* than bt alone (default 7045; iter-off → **6951**, −1.3%).
2. **bt at q11**: covered entirely by the hqdict candidate. 11-file
   q11 sweep with bt off = byte-identical. At q10 bt IS rfc's winner
   (without it 7622 vs 7045) — keep for q10.

Combined skip on rfc q11: size held at 6637, time 7.05s/40 → 2.36s/40
(**−66.5%**). Quiet-box T 4.0 → **1.99**, I 4.1 → **2.0**.

## Change

`omnizip-brotli/src/from_spec_encoder.rs`:

- `run_bt`: default `is_text_like && quality < 11` (q10 only).
  `BROTLI_BT_TEXT=1` forces on at q11; `=0` forces off everywhere.
- iter candidate: default OFF; `BROTLI_ITERCAND=1` restores.
  (Previously `!BROTLI_NO_ITERCAND`.)

## Verification

- q10/q11 corpus: q11 sizes held exactly; q10 rfc 7045→6951 (strict
  improvement), every other file held. Round-trips via `brotli -d`.
- Timing (load ~7–8, /usr/bin/time user): rfc q11 T **1.99** (was 4.0);
  sqlite q11 T **2.46** (was 3.0); install 1.12; icons 1.24.
- Gates: 104+1 tests, fmt, clippy, regression baseline unaffected.

## Board impact (v31)

rfc brotli q11 I 4.1→**2.0** (no longer the worst cell, no longer >4);
sqlite q11 I 3.0→**2.4**. **Zero cells >4 for the first time since the
honest v26 board.** Median holds; ≤3 count rises.
