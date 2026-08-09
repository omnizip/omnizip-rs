# 237 — Determinism Cross-Platform Audit

- **Priority:** P2 (LimniFS hard requirement)
- **Crate:** workspace-wide
- **Depends on:** none
- **Estimated effort:** 2 days

## Goal

Verify and enforce that all encoders produce byte-identical output for the
same input + level across runs, machines, Rust versions, and platforms
(linux, macOS, Windows, x86, ARM).

## Background

LimniFS uses `DropId = BLAKE3(plaintext)` for content-addressed dedup.
Codec non-determinism breaks dedup. The workspace already enforces:
- No `HashSet`/`HashMap` iteration in encode paths
- No time-seeded RNGs
- No thread-scheduling-dependent block boundaries

But determinism has not been formally verified across platforms.

## Potential non-determinism sources

1. **Float arithmetic**: Any `f64` computation that differs by ULP across
   platforms (e.g., `log2`, entropy calculations). Must use integer-only
   approximations or `OrderedFloat`.

2. **Iterator order**: `HashMap`/`HashSet` iteration is non-deterministic
   (random seed per process). Must use `BTreeMap`/`BTreeSet` or sorted
   vectors in encode paths.

3. **Pointer-sized integers**: `usize` differs between 32-bit and 64-bit.
   Any `usize` in encoding decisions must be explicitly cast.

4. **SIMD**: `std::simd` behavior may differ across targets. Must ensure
   SIMD paths produce identical output to scalar paths.

5. **Build configuration**: `cfg` flags that change encoding behavior
   (e.g., `#[cfg(target_arch = "x86_64")]`).

## Plan

1. Create a determinism test suite with fixed inputs at each quality level
2. Generate reference outputs on the canonical platform (linux x86_64)
3. CI runs determinism tests on all platforms (macOS ARM, Windows x86_64,
   linux ARM)
4. Any divergence blocks merge

## Acceptance criteria

- [ ] Determinism test fixtures committed (inputs + expected outputs)
- [ ] CI runs determinism checks on all platforms
- [ ] No `f64` in encode paths (or wrapped in `OrderedFloat`)
- [ ] No `HashMap`/`HashSet` in encode paths
- [ ] No platform-specific `cfg` in encode paths
- [ ] Determinism audit report filed in `docs/determinism-audit.md`
