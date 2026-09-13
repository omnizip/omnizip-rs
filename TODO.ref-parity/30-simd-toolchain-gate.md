# Task 30 — the per-op frontier is toolchain-gated (std::simd audit)

Status: closed (audited 2026-09-13; blocked on Rust stabilization —
not on effort or evidence)

## The question

Tasks 25/29 both bottomed out at per-op constants (the brotli text
matcher at ~92 ns/pos vs the reference's ~24; the zstd fast-tier
emission walks). The documented escape hatch was "the std::simd
program" (task 32 in the original TODO board: `#![forbid(unsafe_code)]`
stands, SIMD via `std::simd`, never raw unsafe). This task asked
whether that program can start today.

## The audit

1. **`std::simd` is still unstable** — tested on the toolchain this
   repo pins (rustc 1.98.0 stable, 2026-08-18): `use std::simd::u8x16`
   → `error[E0658]: unstable library feature 'portable_simd'`. The
   program is blocked on `portable_simd` stabilization, not on us.
2. **The auto-vectorization fallback is exhausted**: the hot loops are
   already in their LLVM-friendly shapes — `match_len_scan` is the
   word-stepped form (u32 first-load + `chunks_exact(8)` u64 stepping
   + byte tail; the u128 variant measured 2x SLOWER on rep-probe-heavy
   text), the bank scan is the macro-inlined two-segment iteration
   (.74-era), the rep probes are the unrolled exact-rep form with the
   reject-byte gate. Remaining scans are data-dependent early-exit
   loops — not vectorizable shapes.
3. The alternative (raw `unsafe` SIMD) violates a workspace hard
   invariant (`#![forbid(unsafe_code)]` in every crate) — not a lever.

## Standing

The hot columns' residual (rustsrc/words brotli q5 ≈ 3.7–3.8, words
zstd L1 3.7, fits brotli q11 3.4) is the price of the safe-Rust
mandate until `portable_simd` stabilizes. Reopen condition: Rust
release notes announcing `portable_simd` stable — then task 32's
original scope applies (matcher scan, FSE state stepping, histogram).
No board cell moves without it.
