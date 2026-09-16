# Task 44 — the portable_simd canary's polarity was broken as a signal

Status: done (2026-09-16)

## The bug

Task 30's canary ran with `continue-on-error: true`: the JOB failed
while std::simd is unstable (E0658), but the RUN conclusion read
"success" in BOTH states — unstable and stabilized. The weekly cron's
green run was identical whether or not the campaign's main unlock had
arrived, and a run-level grep for "success" reads as flipped (it
fooled the terminal audit on 2026-09-15 — the "green" run hid a
failed job).

## The fix

`.github/workflows/simd-gate.yml` inverts the polarity: unstable
(expected) → green run; **compiles → RED run** with an error
annotation naming the reopen procedure (task 30 + the ~15 unlocked
loops + the 21 gated board cells). The red weekly run is the alarm.

## Standing

portable_simd remains E0658 on stable (re-verified 1.98.0 locally and
in CI). rustup reports only 1.98.1 as the newer stable — a patch, no
feature stabilization. Board v39 stands: 21 cells >1.3, all confirmed
faithful-tier floors, every other lever measured (tasks 33, 40, 43).
