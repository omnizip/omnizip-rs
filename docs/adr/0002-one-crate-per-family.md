# ADR-0002: One crate per algorithm family

- **Status:** accepted
- **Date:** 2026-07-15
- **Deciders:** Ronald Tse, omnizip-rs maintainers

## Context

The workspace packages many codecs: LZMA, ZSTD, Brotli, LZ4, DEFLATE,
Snappy, BZip2, PPMd, FSST, Rice++, FLAC, BLOSC, GLZA, ZPAQ,
libdeflate, Deflate64. Each has independent feature sets, versioning,
and dependency trees.

Possible layouts:

1. **Monolithic** — single crate with features per codec.
2. **Per-family crate** — one crate per algorithm family.
3. **Per-format crate** — split encoders/decoders into separate crates.

## Decision

**One crate per algorithm family.** Each `omnizip-{name}/` directory
is published as its own crate (e.g., `omnizip-lzma`, `omnizip-zstd`).
The shared trait crate `omnizip-codecs` is the only cross-crate
dependency.

## Consequences

**Positive**:
- **Independent versioning** — bug fix to LZMA doesn't force a
  ZSTD release. Each codec's CHANGELOG is self-contained.
- **Feature flags per codec** — a downstream user of `omnizip-brotli`
  doesn't pay compile cost for ZSTD.
- **Compile parallelism** — `cargo build` compiles all crates in
  parallel; much faster than a monolith.
- **Smaller dependency footprint** —LimniFS can pick the codecs it
  needs without pulling in others.
- **Cleaner public APIs** — each crate has its own `lib.rs` with a
  focused surface area.

**Negative**:
- **Workspace is wide** — 17 crates is harder to navigate than one.
  Mitigated by `omnizip-codecs` providing the dispatch entry point.
- **Cross-crate refactors are slow** — changing a shared trait
  requires touching every impl. Acceptable; trait changes are rare.
- **Workspace metadata drift** — each crate's `Cargo.toml` must
  stay in sync with the workspace. Mitigated by workspace
  inheritance (`version.workspace = true`).

**Neutral**:
- Matches the structure of the Ruby `omnizip` reference (one class
  per algorithm family).

## References

- [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html)
- [Workspace inheritance](https://doc.rust-lang.org/cargo/reference/workspaces.html#the-package-table)
- [Ruby omnizip](https://github.com/omnizip/omnizip) — similar per-
  class layout.
