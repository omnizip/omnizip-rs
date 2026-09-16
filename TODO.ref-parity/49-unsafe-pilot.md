# Task 49 — the unsafe pilot: executed, contracts held, verdict NEGATIVE

Status: closed (2026-09-16; option 4's premise measured dead — the
invariant survives with direct evidence)

## The pilot (owner-approved)

Quarantine design: a dedicated `omnizip-kernel` crate (the ONLY
unsafe-bearing code in the workspace; read-only unchecked loads with
documented safety contracts + debug_assert bounds), wired into the
zstd fast matcher's `read32` sites — each audited as
guard-established (ip2+4<=iend / match_idx+4<=iend / pipeline
positions < ilimit = len-8).

- Contracts verified: the full zstd debug suite (191/191) ran through
  the debug_asserts without a single violation.
- Byte-identity: 90/90 corpus cells identical to main at
  L1-L6/L9/L12/L19.
- Timing (loop canon, load 47-52): words L1 3.76->3.75s, fits
  2.63->2.68, csv2m 2.89->2.89, rustsrc 3.69->3.62, plists
  2.17->2.20 — **flat within noise**.

## Why (the refined diagnosis)

LLVM already fuses the adjacent guard+load pairs — the bounds checks
the theory targeted were not being emitted at those sites. Combined
with task 40/46/47's results, the residual per-op gap on the
sequential loops is now attributed to pipeline codegen shape
(register pressure across the transliterated state machine), which
has neither a safe-Rust lever nor an unsafe lever short of
hand-scheduling the loop. The seventh and final confirmation.

## Consequence

Option 4 (lifting #![forbid(unsafe_code)]) is withdrawn from the
choice set: measured to buy nothing on the pilot loop, so there is no
reason to believe it buys anything on its siblings. The frontier
(task 48, corrected) reduces to: accept the floor, the down-tier
band, or raw-block. REVERTED entirely — no unsafe code ships; the
workspace invariant is intact and now evidence-backed.
