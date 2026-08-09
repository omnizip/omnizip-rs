# 236 — Code Review Sweep (OCP / MECE / DRY)

- **Priority:** P3 (architecture quality)
- **Crate:** workspace-wide
- **Depends on:** [233](233-shared-match-finder-abstraction.md),
  [234](234-shared-bitstream-module.md)
- **Estimated effort:** 2 days

## Goal

Systematic code review sweep across all workspace crates to enforce OCP,
MECE, and DRY principles. Identify and fix architectural issues that
hinder maintainability and extensibility.

## Checklist per crate

### OCP (Open/Closed Principle)
- [ ] Adding a new codec = one new crate + one register() call (no dispatch changes)
- [ ] Adding a new filter = one new module + one register() call
- [ ] Adding a new strategy = one new enum variant + one match arm (no if-else chains)
- [ ] Configuration changes don't require modifying core logic

### MECE (Mutually Exclusive, Collectively Exhaustive)
- [ ] Each concern lives in exactly one place (no duplicated logic)
- [ ] No gaps in responsibility (every operation has a clear owner)
- [ ] Module boundaries align with domain concepts

### DRY (Don't Repeat Yourself)
- [ ] No duplicated match finder code (use shared abstraction)
- [ ] No duplicated bit writer code (use shared bitstream)
- [ ] No duplicated Huffman builder code (use shared package-merge)
- [ ] No duplicated hash function (use shared xxhash/hash4)

### Code quality
- [ ] No `unsafe` blocks (workspace-wide `#![forbid(unsafe_code)]`)
- [ ] No `unwrap()` outside tests (use proper error handling)
- [ ] No magic numbers (all constants named and documented)
- [ ] All public APIs documented with examples

## Process

1. `cargo clippy --workspace --all-targets -- -D warnings` — fix all warnings
2. Manual review of each crate's `lib.rs` for API surface quality
3. Check for dead code (remove or document as intentionally retained)
4. Verify error messages are actionable (include context for debugging)
5. Audit `unsafe` usage (should be zero — `#![forbid(unsafe_code)]`)

## Acceptance criteria

- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] No duplicated logic across crates
- [ ] All public APIs have doc comments
- [ ] Code review document filed in `docs/code-review-sweep.md`
