# ADR-0004: Determinism as a hard requirement

- **Status:** accepted
- **Date:** 2026-07-15
- **Deciders:** Ronald Tse, omnizip-rs maintainers

## Context

The primary consumer is [LimniFS](https://github.com/limnifs/limnifs),
a content-addressed filesystem where `DropId = BLAKE3(plaintext)`.
To dedup compressed blobs across machines and runs, the **same input
+ same level** must produce **byte-identical compressed output**
across:

- Different machines (different CPU, different memory layout)
- Different Rust versions (stable 1.75 → 1.80+)
- Different OSes (Linux, macOS, Windows)
- Different build flags (debug vs. release)
- Different times of day (no time-seeded RNG)
- Different thread schedules (no concurrency-dependent iteration)

If determinism breaks, dedup misses collapse. Two identical files
become two different compressed blobs.

Common determinism-violators in compression code:

- **`HashMap`/`HashSet` iteration** — order is randomized per-run
  via `RandomState`.
- **Time-seeded RNGs** — `thread_rng()` or `SystemTime::now()` in
  encoder paths.
- **Pointer addresses** — hashing or ordering by `&T` address.
- **Float operations** — IEEE 754 allows different rounding on
  different ISAs (x87 vs. SSE vs. ARM).
- **Thread pool work order** — parallel reduction that depends on
  per-thread scheduling.

## Decision

**Determinism is a hard requirement for every encoder.**

Enforced by:

1. **CI checks**: `cargo test --workspace --test determinism` runs
   each encoder 5× in the same process; asserts byte-identical.
2. **Cross-platform check**: same input compressed on Linux +
   macOS in CI; outputs diffed.
3. **Lint rules**: PRs adding `HashMap`/`HashSet` to encode paths
   are flagged in code review.
4. **Documentation**: each encoder's doc comment must state the
   determinism guarantee.
5. **No `unsafe`** (per ADR-0001): removes UB-related determinism
   issues.

## Consequences

**Positive**:
- LimniFS dedup works as designed.
- `git blame` on compressed blobs is meaningful — the byte sequence
  is reproducible from the input.
- Bugs are easier to reproduce — a failing test always fails the
  same way.

**Negative**:
- **Performance ceiling**: some fast algorithms are non-deterministic
  (e.g., parallel reduction with thread-pool-dependent order). We
  use sequential versions where the speedup isn't worth breaking
  determinism.
- **`HashMap` replaced with `BTreeMap`/`IndexMap`** in some hot
  paths; small perf cost.
- **`thread_rng()` replaced with seeded `ChaCha8Rng`** in the rare
  case where randomness is needed (e.g., fuzz tests).
- **Float ops avoided in encode paths**: we use integer arithmetic
  for Huffman code lengths, hash chains, etc.

**Neutral**:
- All known encoders in omnizip-rs satisfy this. The determinism
  tests catch regressions.

## References

- [LimniFS](https://github.com/limnifs/limnifs) — content-addressed FS
- [`tests/determinism/`](../../tests/determinism/) — workspace-wide
  determinism test suite.
- [Convergent encryption](https://en.wikipedia.org/wiki/Convergent_encryption)
  — the cryptographic pattern LimniFS uses.
