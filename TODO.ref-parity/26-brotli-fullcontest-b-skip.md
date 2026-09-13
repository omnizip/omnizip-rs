# Task 26 — brotli: binary-class b-skip in the full contest's conditional pass

Status: done (2026-09-13, shipped v0.21.88)
Board cells: sqlite brotli q11 3.5

## Finding

Task 16 gated the *reduced* contest's b (split literal assignment)
emission on `is_text_like` — but small+dense inputs take the *full*
contest path, whose conditional split pass ("measure b for the best
candidate and the runner-up within 1%") still ran unconditionally. The
one dense-binary full-contest input (sqlite — its DB text passes the
density screen) paid one b emission per chunk that never wins
(BTOPT_DUMP: hq 331,981 with split=false; b lost).

## Change

`omnizip-brotli/src/from_spec_encoder.rs`: the conditional split pass
is gated on `is_text_like(input) || BROTLI_SPLITCAND_ALL` — same
winner-table evidence and env restore as task 16's reduced-path gate.

## Verification

- Corpus q10 + q11: all sizes byte-identical (every value matches the
  v0.21.82-era board numbers exactly); round-trips via `brotli -d`.
- Timing (interleaved /usr/bin/time user, load 8.75, 3/3 rounds
  consistent): sqlite q11 11.67/11.70/11.86 → 10.26/10.48/10.55 =
  **−10.5%**. T 3.38 → ~3.0, I 3.5 → ~3.1.
- Gates: 104+1 brotli tests, fmt, clippy (CI invocation).

## Residual

sqlite q11's remaining structure: full contest on the dense-small
class (hq + hq_d + iter parses, a + treecap emissions) — the same
deliberate trade as rfc q11 (task 18), now with every binary-only
waste gated (bt by `run_bt`, b by tasks 16+26).
