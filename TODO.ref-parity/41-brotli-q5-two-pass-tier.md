# Task 41 — brotli q5 → the two-pass fragment tier (the authorized size-for-time trade)

Status: done (2026-09-15, shipped v0.21.96)
Board cells: the entire brotli q5 column (10 tracked cells, I 2.1-2.5
→ **0.12-0.43**)

## The decision

Task 40 deferred this as an owner call; the owner said proceed. The
from-spec greedy tier at q5 measures I 2.1-2.5 on every board cell
(the ~92ns/pos safe-Rust floor vs the reference's ~24ns/pos H6 — no
per-position tier crosses 1.3, confirmed across q3/q5 shapes, bank
depths, and chain configs in task 40). The two-pass fragment tier
(q1's algorithm, transliterated BrotliCompressBlockFast) runs at
memory speed:

| cell | two-pass I | (was) |
|---|---|---|
| fits | 0.12 | 1.7 |
| words | 0.19 | 2.2 |
| csv2m | ~0.2 | 2.1 |
| rustsrc | 0.43 | 2.2 |
| dbdump/plists/noto/sqlite/install/icons | 0.26-0.35 | 2.3-2.35 |

(small-file T measured with the loop canon, both sides.)

## The cost (the trade, stated plainly)

q5's output is now byte-equal to q1's (verified 11/11 files): sizes
S 1.15-1.70 vs the greedy tier's 0.80-1.01. q5 is no longer smaller
than q4 — the tier's semantic changed. The ladder is now: q1-5
fragment-class speed with q6-9 greedy and q10-11 the zopfli contest.
BROTLI_NO_TP_Q5 restores the greedy tier.

## Change

`omnizip-brotli/src/from_spec_encoder.rs`: q5 routes to
`compress_two_pass_q1` (one branch, env-restorable). Regression
baseline refreshed (documented process; q5 fixtures now pin the
two-pass sizes).

## Verification

- 11/11 files: q5 output byte-equal to q1, round-trips via the
  reference decoder.
- Gates: 104+1 brotli tests, regression suite green on the refreshed
  baseline, fmt clean.
- Board v37: q5 column re-scored from the measured T/S above.

## Residual

q4 keeps the greedy tier (untracked; owner may want the same flip for
ladder coherence — q4 is now both slower AND smaller than q5). q9
words 1.5 / q11 sqlite 2.3, noto 1.96, rfc ~2.0 remain at the
per-op floor (tier-flipping THOSE would be I-gaming — their product
value is size; not done without an explicit call).
