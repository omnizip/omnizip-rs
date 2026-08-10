# ADR-0009: Workspace lints (clippy, forbid unsafe)

- **Status:** accepted
- **Date:** 2026-07-15
- **Deciders:** Ronald Tse, omnizip-rs maintainers

## Context

A 17-crate workspace without lint discipline becomes inconsistent
fast: one crate uses `unwrap()`, another `expect()`, a third
propagates `Result`. Reviewers can't catch every regression by eye.

The alternatives:

1. **No workspace lints** — each crate self-governs. Inconsistent.
2. **Workspace `clippy::pedantic` = "warn"** — very strict,
   generates noise on legitimate patterns (e.g., cast lints in
   bit-twiddling code).
3. **Per-crate opt-in** — crates choose their strictness. Loses
   workspace-wide enforcement.
4. **Minimal workspace lints + per-crate opt-in to pedantic** —
   strict only where the crate wants it.

## Decision

**Workspace-level**: minimal lints (`unsafe_code = "forbid"`,
default warnings). **Per-crate**: opt into `clippy::pedantic =
"warn"` or `clippy::restriction` as needed via crate-level
attributes.

```toml
# Cargo.toml (workspace root)
[workspace.lints.rust]
unsafe_code = "forbid"

# Workspace clippy config: deliberately minimal. The CI runs clippy
# with -D warnings; individual crates can opt into stricter lints via
# crate-level attributes.
[workspace.lints.clippy]
```

```rust
// In a crate that wants stricter lints:
#![warn(clippy::pedantic)]
#![allow(clippy::cast_possible_truncation)]  // bit-twiddling code
#![allow(clippy::similar_names)]  // domain-named variables (e.g., dist_rb_idx)
```

## Consequences

**Positive**:
- **`unsafe` is impossible**: every crate inherits the forbid.
- **CI catches warnings**: `cargo clippy -- -D warnings` fails the
  build on any warning.
- **Crate-level flexibility**: a codec with bit-twiddling can
  allow `cast_possible_truncation`; an API crate can be stricter.
- **No workspace-level lint-group conflicts**: setting pedantic
  at workspace level causes priority conflicts with crate allows.

**Negative**:
- **Inconsistency across crates**: one crate may have stricter
  lints than another. Acceptable; the strictness matches the
  crate's domain.
- **New lint rules require crate-level edits**: when clippy adds a
  new pedantic lint, crates that opted into pedantic will get the
  new warning. Usually desirable but can cause CI surprises.
- **`cast_possible_truncation` is real noise** in codec code that
  intentionally narrows `u64` hashes to `u32`. Mitigated by
  targeted allows.

**Neutral**:
- `cargo fmt --all -- --check` is enforced via CI separately.

## References

- [Workspace lints](https://doc.rust-lang.org/cargo/reference/workspaces.html#the-lints-table)
- [Clippy lint groups](https://rust-lang.github.io/rust-clippy/master/)
- [Crate-level lint attributes](https://doc.rust-lang.org/rustc/lints/levels.html)
