# ADR-0001: Pure-Rust only (`#![forbid(unsafe_code)]`)

- **Status:** accepted
- **Date:** 2026-07-15
- **Deciders:** Ronald Tse, omnizip-rs maintainers

## Context

Compression codecs traditionally bind to C reference implementations
(`xz-utils`, `zstd`, `brotli`, `lz4`). C bindings via FFI introduce
several costs:

- **Auditability** — `unsafe` blocks hide memory-safety issues. A
  buffer overflow in the C reference is a Rust vulnerability.
- **WASM/embedded** — C toolchain isn't always available. Cross-
  compilation to `wasm32-unknown-unknown` or `armv7-unknown-none-eabi`
  breaks when a C dependency is added.
- **Determinism** — C compilers vary across platforms; the same code
  compiled with different `-O` flags can produce different binaries
  due to undefined behavior. LimniFS requires byte-identical output
  across machines for content-addressed storage.
- **Build complexity** — `cc` crate, `bindgen`, `pkg-config`, and
  system-library headers add build-time fragility.

The alternatives:

1. **FFI to C references** — fastest path to correctness, accepts
   the costs above.
2. **Pure Rust, allow `unsafe`** — middle ground; faster than safe
   Rust for SIMD, but loses auditability.
3. **Pure Rust, `#![forbid(unsafe_code)]` workspace-wide** — most
   conservative; relies on `std::simd` (still stabilizing) for any
   vectorization.

## Decision

omnizip-rs is **pure Rust, workspace-wide**:

```toml
# Cargo.toml (workspace root)
[workspace.lints.rust]
unsafe_code = "forbid"
```

Every crate inherits this. No `unsafe` blocks, no `unsafe fn`, no
raw pointer derefs outside of well-vetted standard library code.

## Consequences

**Positive**:
- All memory-safety bugs are compile-time errors.
- Auditors can `grep -r "unsafe" omnizip-*` and get zero hits.
- Cross-compiles to any Rust target without a C toolchain.
- Output is more portable across OS/compiler versions (Rust has
  fewer UB pitfalls than C).
- Codecs can be used in WASM modules, embedded firmware, and
  safety-critical contexts without review burden.

**Negative**:
- Performance ceiling is lower than hand-tuned C with intrinsics.
  Mitigated by `std::simd` (where stable) and the `wide` crate
  (used for SIMD Huffman, see TODO 102).
- Re-implementing battle-tested algorithms (LZMA range coder,
  ZSTD FSE) introduces wire-format bugs that the C reference
  doesn't have. Mitigated by differential parity testing.
- Some advanced optimizations (e.g., BMI2 LZMA decoder) are
  unreachable without inline assembly.

**Neutral**:
- `cargo build --workspace` is slower than a single C binary, but
  incremental builds are fast.

## References

- [LimniFS](https://github.com/limnifs/limnifs) — content-addressed
  FS requiring deterministic compression.
- [std::simd](https://doc.rust-lang.org/std/simd/) — portable SIMD
  in stable Rust.
- [Ruby omnizip](https://github.com/omnizip/omnizip) — algorithmic
  reference for ports.
- [`#![forbid(unsafe_code)]`](https://doc.rust-lang.org/nomicon/unsafe.html)
  — the strongest `unsafe` lint level.
