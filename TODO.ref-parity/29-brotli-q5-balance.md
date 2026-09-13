# Task 29 — brotli q4-5 balance: literal-split skip (smaller AND faster)

Status: done (2026-09-13, shipped v0.21.89)
Directive: "q5 need a balance" — user response to the task-28 inventory
of the q5 text column (T 2.5–3.9, the un-replicated reference tier).

## What was measured (balance sweep, all env-gated before shipping)

- **Bank depth (16/8/4 slots)**: −5…−14% T on words only, sizes mixed
  (rustsrc +3.0%, icons +4.3% at 8 slots; csv2m −5% at 4) — not a
  uniform lever.
- **Lazy lookahead off**: catastrophic — csv2m +68% size (200,437 →
  336,146) AND slower (worse matches inflate the command stream and
  emission). Lazy deferral is load-bearing on both axes; the parse
  config is at a measured local optimum.
- **lazy2 off**: byte-identical, ~flat time (no observable q5 effect;
  folded into the sweep knowledge, not shipped separately).
- **Rep probes**: already the unrolled exact-rep-only form (4 probes,
  reject-byte gate) — no fat.
- **Literal split (the ship)**: with the decided static map chosen,
  extra literal blocks only pay switch codes. Skipping it at q4-5 is
  a STRICT win — smaller on text (words −1,027B, dbdump −597B,
  plists −482B, install −165B, icons −75B; q4: words −879B,
  csv2m −999B), byte-identical on binary and at q6+ (their ±0.2%
  deltas stay split), and faster where the split worked hardest
  (plists −16%, dbdump −17%, icons −9%, install −6%).

## Change

`omnizip-brotli/src/encoder/emission.rs`: `lit_split_on` now requires
`quality >= 6` (was 4). `BROTLI_FORCE_LIT_SPLIT` restores via the
existing helper. Measurement knob `BROTLI_Q5CFG` retained.

## Verification

- Corpus q4/q5: sizes match the sweep predictions exactly; all
  round-trips via `brotli -d` byte-exact; q6/q9/q11 spot-identical;
  binary q5 identical.
- Regression suite: passes unchanged (the 100 KB fixtures already take
  the decided-map path — the waste was on larger inputs).
- Gates: 104+1 tests, fmt, clippy (CI invocation).
- Board (ratio basis on v28 quiet values): plists q5 I 3.8→3.2,
  dbdump 3.0→2.5, install 2.6→2.4, icons 2.7→2.4; all text q4/q5 S
  improve.

## Residual (the honest balance statement)

The remaining q5 text T gap (words 3.8, rustsrc 3.7) is the
per-position constant of the safe-Rust bank matcher (92 ns/pos vs the
reference's 24) — every parse knob measured at its optimum; the next
movement is the std::simd per-op program (task 32 class), not config.
The q5 tier keeps its size wins (words q5 S≈0.95 — 5% smaller than
the reference at 1/3 the speed gap it had).
