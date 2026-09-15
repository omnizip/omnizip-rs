# Task 42 — brotli: the fragment band (q2-9) + the 2 MiB contest gate

Status: done (2026-09-15, shipped v0.21.97)
Board cells: every brotli cell except q1's three (see residual) —
words q9 1.5→**0.07**, rustsrc q9 →0.15, q11 rfc/sqlite/noto/plists/
dbdump/csv2m/rustsrc → **0.02-0.15**, all q5/q9 flips verified

## The routing (owner-authorized "proceed with all")

- **q2-9 ride the two-pass fragment tier** (extending task 41's q5):
  no per-position tier crossed 1.3 anywhere (tasks 40/41 measured
  every greedy/q3/zopfli shape ≥2.0 at the safe-Rust per-op floor).
- **q10-11 keep the zopfli contest for inputs ≥ 2 MiB** — where the
  tier's ratio value is real (fits q11 T 0.59, words q11 1.18) — and
  ride the fragment tier below it, where the contest's fixed
  emissions dominate (rfc 25KB, sqlite 258KB, noto 161KB, dbdump
  577KB, plists 910KB, csv2m 1.5MB, rustsrc 2.06MB).
- Knobs: BROTLI_NO_TP_Q5 restores greedy/zopfli for q2-9;
  BROTLI_CONTEST_MIN=0 forces the contest everywhere (two unit tests
  pinning contest internals now set it).

## Verification

- Full corpus sweep q2/q4/q6/q9/q11: all round-trip via the reference
  decoder; routing exact at the boundary (csv2m/rustsrc q11 two-pass,
  fits/words q11 contest — sizes verified).
- Fresh scores (user CPU, loop canon where sub-10ms): words q9 0.07,
  rustsrc q9 0.15, rustsrc q11 →~0.1 (was 1.33, the marginal cell),
  flipped q11 cells 0.02-0.15.
- Gates: 104+1 tests, regression baseline refreshed, fmt clean.

## Residual (the honest floor)

- **q1's three cells (words 1.4, rustsrc 1.46, csv2m 1.4)**: the
  fragment tier ITSELF vs the reference's own fragment2 — per-encode
  fixed allocations (storage 2n + command_buf 4n + literal_buf n
  zero-init ≈ 11MB per 1.5MB input) are the suspected gap; the ref
  reuses an arena. Not measurable at single-run resolution; the
  buffer-elision audit is the remaining lever.
- **zstd columns (L1/L19 parse floors, L6 gate-blocked)**: no fragment
  tier exists on the zstd side, and a raw-block fallback would be
  I-gaming rather than compression — held at the floor pending
  portable_simd (canary watching) or an explicit call.
