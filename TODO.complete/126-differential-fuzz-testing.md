# TODO 126: Differential fuzz testing

## Problem

The current differential harness runs the Ruby reference + C
binaries on a fixed corpus. It catches regressions on known inputs
but not on the long tail of edge cases that fuzzing would find.

## Proposed fix

Add a `cargo fuzz`-style runner that:

1. Generates random inputs of varying compressibility via property
   strategies:
   - Pure-noise (incompressible)
   - Highly-repetitive (periodic)
   - Natural text (enwik-like)
   - Binary with long runs
   - Adversarial patterns (e.g., the input that triggered the L12
     regression)
2. For each input:
   - Encode via Rust codec.
   - Decode via Rust codec → assert byte-exact.
   - Decode via reference C/Ruby tool → assert byte-exact.
3. If a round-trip fails, save the input + outputs to `fuzz-corpus/`
   and exit non-zero so CI catches it.

## Acceptance criteria

- [ ] `cargo run --example fuzz-differential` runs for ~60 s and
  catches any failure on at least 1000 random inputs per codec.
- [ ] Repro artifacts saved to `fuzz-corpus/{codec}/{timestamp}.bin`
  on failure.
- [ ] CI runs a 5-minute subset of this on every PR.

## Priority

P1 — without fuzzing, regressions like TODOs 110 and 121 keep
slipping through manual testing.
