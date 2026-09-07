# 05 — csv2m-class periodic data: cross-codec time pathology

- **Priority:** P0 (contains the single worst cell on the v3 board)
- **Score evidence (v3, amortized both sides):** csv2m zstd L6
  **I=137** (T=202x, S=0.681); csv2m zstd L1 I=34 (T=34x); csv2m
  zstd L19 I=11; csv2m brotli q1 I=24.5 (T=29x).
- **Status:** root cause 1 (getenv) FIXED 2026-09-07; root cause 2
  (structural tier routing) moved to task 03 — see "Outcome".

## Root cause — what it actually was

The file-periodicity hypothesis below was WRONG for the dominant
factor. Sampling the csv2m zstd L6 encode showed **~53% of samples
inside `getenv`**: `insert_bt_and_get_all_matches`
(omnizip-zstd/src/encoder/opt.rs) called
`std::env::var_os("ZSTD_OPT_DUMP")` at EVERY match-finder position,
and `getenv` takes the global environ lock each call — the same trap
brotli's decode loop hit on 2026-08-19. Periodicity only made it
worse by maximizing positions-per-byte-throughput of the DP.

### Fix (v0.21.66)

```rust
fn opt_dump_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZSTD_OPT_DUMP").is_some())
}
```

- csv2m zstd L6: **3.09s -> 1.71s** best-of-3 (T: 202x -> ~112x
  amortized), size unchanged (221,185). Output-neutral: no baseline
  refresh needed.
- Every opt-routed zstd cell (L4-19) benefits; partial re-run under
  box load ~246 showed rfc L6 0.0175 -> 0.0062s (2.8x) but noisy
  inflation elsewhere — full board re-score DEFERRED until the shared
  box is quiet (README principle). Pre-fix numbers are in
  `sweep-inequality-v3-2026-09-07.txt`.

## What remains (moved to task 03)

The residual ~20-30x T on L6 cells is STRUCTURAL: block.rs routes
ALL strategies >= DoubleFast (i.e. every level >= 4) to the opt
parser — a deliberate size-parity choice predating this board. The
reference runs dfast/lazy/btlazy2 at L4-12. The `uses_opt` threading
in write_block_cross (this release) makes the tier routing a one-line
change; the probe trade on csv2m: fast4 fallback 300,934B/0.074s vs
opt 221,185B/1.71s vs ref 324,867B. Porting real dfast/lazy
parse shapes = task 03.

csv2m zstd L1 I=34 and brotli q1 I=24.5 are separate (fast-parser
tier and two-pass stored-block path) — tracked under tasks 03/02.

## Original hypothesis (kept for the record — disproven)

Periodic data produces near-perfect matches at every position; the
zopt-style DP/bank scoring re-evaluates long candidate chains per
position (the 0.16.x-era zeros-quadratic class). The reference's
tiered hashers (dfast/lazy/greedy) have per-position work that is
BOUNDED regardless of match quality. — Profiling showed the DP itself
was NOT the hot spot once getenv was hoisted; the periodic-content
bounded-work question folds into task 03's port (bring the reference's
skip guards with the tier shapes).

## Acceptance

- ~~csv2m cells: zstd L1/L6/L19 and brotli q1 all at I <= 10~~ —
  amended: this task delivers the getenv fix (opt-routed cells ~2x
  time); the I<=10 target for L6/L1 requires task 03's tier port.
- Periodic-structured-text regression fixtures still pass; regression
  gate green; byte-determinism green (output unchanged).
