# Task 59 — progress / ETA reporting

Status: open (part of task 51 phase 2; wires into 57 streaming and
58 parallel — design the trait first so those tasks can call it)

## Gap

Ruby `progress/` + `eta/`: `progress_reporter` with
silent/callback/console/log implementations, `progress_tracker`
(rate accounting), `progress_bar`, `operation_progress`, and ETA
estimation from moving rates. Rust: nothing — long operations are
opaque.

## Scope

- `trait ProgressReporter: Send + Sync` in omnizip-codecs with
  `Silent` no-op default:
  `fn on_progress(&self, op: &Operation, done: u64, total: u64)`
  plus `on_start/on_finish/on_error`. `Operation` names
  compress/decompress/verify/extract/create/convert + subject path.
- `ProgressTracker`: exponential-moving-average rate + ETA
  (`eta/` semantics: elapsed, remaining, smoothed rate — no wall
  clock in any OUTPUT decision, reporting only).
- `ConsoleReporter` (stderr, no ANSI beyond optional bar — crate
  stays dependency-free) and `CallbackReporter` (generic closure —
  this is what ozip and the future Ruby FFI layer will use).
- Wire points: archive create/extract loops (entries + bytes),
  streaming push/finish (task 57), parallel engine job completions
  (task 58). Reporting is best-effort: failures in a reporter never
  fail the operation.

## Acceptance

- CallbackReporter captures a monotonically increasing `done` ≤
  `total` sequence for every wired operation (property test).
- ETA math unit tests with injected rates (no real sleeps beyond
  one short integration case).
- Zero behavioral impact with Silent (existing suites unchanged,
  byte-identical outputs — reporting MUST sit outside data paths).

## References

`../omnizip/lib/omnizip/progress/*.rb`, `../omnizip/lib/omnizip/eta/`.
