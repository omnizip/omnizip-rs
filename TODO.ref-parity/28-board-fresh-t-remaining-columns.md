# Task 28 — board hygiene II: fresh-T for the remaining columns (brotli q1/q5, xz-6)

Status: done (2026-09-13)
Trigger: task 24's sweep covered zstd{1,6,19} + brotli{9,11} only; the
board's brotli q1, brotli q5, and xz-6 columns (33 cells) still carry
pre-canon T values from the methodology task 24 proved systematically
wrong (ref-side date-granularity artifacts — the q9 column was
under-measured by ~2x).

## Sweep

Same harness as task 24 (`/usr/bin/time` user CPU both sides, static
per-cell N ≥ 2s, ref at 2×N, load 5–6 throughout). xz reference runs
`xz -6 -T1` to pin single-thread parity with our in-process codec.

## Results (33 cells, load 4-7)

**21 cells re-scored** (brotli q1 + brotli q5); the xz-6 column's
carried values already matched the fresh readings — our xz is at
reference parity (T 1.07–1.35, rfc 2.0), no movement.

- **brotli q1**: 1.05–1.46 — the carried 0.0–0.6 lows were artifacts
  of the old methodology.
- **brotli q5**: text cells 3.8 (plists/words/rustsrc), csv2m 2.5,
  dbdump 3.0, install/icons 2.6 — the greedy tier's deliberate
  ratio-for-time trade (task 01) honestly visible for the first time;
  the binary cells hold 1.5–1.9.
- Board v28: **median I=1.40, 71/77 ≤3**, one cell >4. All 77 cells
  are now canon-measured — no pre-task-24 value survives anywhere in
  the table.

The q5 text trio (I 3.8) is the reference's H6 lazy+hasher tier that
task 01 deliberately did not replicate (it exists to WIN size:
words q5 S≈0.95). Reopen condition unchanged from task 01: a
user decision to trade the size wins for time.
