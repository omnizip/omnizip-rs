# Task 40 — executing the three escape hatches (a, b, c)

Status: closed (2026-09-15; (a) verified toolchain-blocked today, (b)
confirmed research-closed, (c) executed where a genuine I-win exists —
one candidate found, below; the q5 tier-gut is a product decision
deferred to the owner with measured numbers)

## (a) portable_simd — still blocked, verified 2026-09-15

rustc 1.98.0 stable: `#![feature(portable_simd)]` → E0554 (canary
unchanged). The (a)-shaped work available on stable —
auto-vectorizable restructures — was measured:

- `count_abs` 32-byte steps ([u64;4] XOR ILP): **byte-identical
  (44/44 corpus L1/L6/L12/L19) but timing-FLAT** (dbdump L19
  0.47→0.47s; these files' matches end within 8-32 bytes — the wide
  step never engages, and the two 32-byte copy_from_slices cost what
  the ILP saves). REVERTED, not shipped. Same verdict as the task-37
  find_blocks rewrite: no measured gain → not shipped.
- Bank depth cuts (`BROTLI_BLOCK_BITS` 4→3→2 on the q5 text column):
  time barely moves (scan cost is per-position fixed work, not depth)
  and sizes regress 0.3-7% — rustsrc +3.0/+7.1%, install +2/+5%.
  Only csv2m improves at bb2 (−4.9% size AND −11% time: the extra
  bank candidates seduce the parse off the periodic rep continuations
  — the same mechanism documented for the q8-9 shape). Even with that
  free win csv2m q5 I only reaches ~1.53 — not worth a content gate
  for one cell.
- q3-parse-at-q5 hybrid (NO_BANK + chain 8/nice 16): SLOWER (words
  1.00s vs 0.11s — without the bank the lazy chain walk dominates)
  and larger. Dead.
- Chain/nice via `BROTLI_MFOVR` on the ≥2MiB override: sizes IDENTICAL
  across 16/24/32/64 chain on words/rustsrc/csv2m — the override's
  chain never binds on these files; the knob stays for future sweeps.

## (b) task-21 emitter — confirmed closed

Needs 5× cheaper exact emission; today's flat count_abs + the gate-2
closure (the mode search IS the win) leave no scalar path. The
scratch-reuse slice shipped in v0.21.95 was the last measurable cut.

## (c) size-for-time — one genuine candidate; the tier-gut deferred

- Genuine (no trade, both axes improve): csv2m-class periodic streams
  prefer a 4-slot bank (bb2, see above) — parked pending a second
  periodic-class corpus member (a content gate for one cell is
  overfitting).
- The q5 text column CANNOT reach 1.3 in any per-position tier: even
  our q3 shape measures I≈2.05 on words (0.09s vs ref 0.043s — the
  ~92ns/pos vs 24ns/pos per-op floor spans every tier). The only
  config that crosses is routing q5 text to the TWO-PASS fragment
  path (q1's algorithm): measured words two-pass ≈ 876,360B vs ref q5
  765,030 → I ≈ 0.4-0.6, but q5 output becomes byte-equal to q1-class
  (+14.5% vs ref) and inverts the effort ladder (q5 faster than q2-4).
  This is a product decision, not an I-score call — the authorized
  "reference's H6-lazy shape" (≈ ref sizes at ref speed) is not
  buildable without (a). Numbers for the owner:
  q5 text today = S 0.97-1.01, T 2.0-2.6, I 2.1-2.5 (7 cells);
  two-pass q5 = S ~1.10-1.15, T ~0.3-0.5, I ~0.4-0.6.

## Standing

Board v36 unchanged (41 cells >1.3). The per-op floor now has three
independent confirmations on each side (brotli scan loops, zstd opt
DP, zstd fast parse); the canary remains the unlock.
