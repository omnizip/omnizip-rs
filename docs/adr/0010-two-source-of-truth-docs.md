# ADR-0010: Two-source-of-truth docs

- **Status:** accepted
- **Date:** 2026-07-20
- **Deciders:** Ronald Tse, omnizip-rs maintainers

## Context

omnizip-rs documentation grew organically across:

1. **`CLAUDE.md`** (root) — instructions for AI assistants working
   in this repo. Invariants, build/test commands, porting workflow.
2. **`PLAN.md`** (root) — LZMA + ZSTD porting plan (the original
   "what ships when" doc).
3. **`TODO.complete/`** (directory) — MECE task breakdown for the
   whole workspace, with priorities and dependencies.
4. **`docs/`** (directory) — spec compliance notes, architecture
   docs, this ADR directory.
5. **`docs/adr/`** (subdirectory) — ADRs (architectural rationale).
6. **Per-crate `README.md` / `lib.rs` doc comments** — API-level
   documentation.
7. **`BUGREPORT-*.md`** (root) — incident post-mortems.

This is more than one place, and that's intentional. Each
audience needs a different view.

## Decision

**Keep the multi-view structure, with explicit role boundaries.**

| Document | Audience | Content |
|---|---|---|
| `CLAUDE.md` | AI assistants (Claude Code) | operational rules: how to build, test, port, what invariants must hold |
| `PLAN.md` | humans porting LZMA + ZSTD | what to port in what order; per-task Ruby → Rust file mapping |
| `TODO.complete/` | contributors | per-task specs with acceptance criteria; status tracking |
| `docs/` | spec-compliance readers | deep dives on wire-format details, edge cases, references |
| `docs/adr/` | all readers | why decisions were made; immutable record |
| `lib.rs` | API callers | how to use this crate; types, traits, examples |
| `BUGREPORT-*.md` | incident investigators | what went wrong, how it was found, how to prevent |

**Drift management**: when one document changes, the others may
need updating. Enforced by:

- PR templates include a "docs checklist".
- `CLAUDE.md` includes a list of docs that must be updated when
  adding new TODOs or changing architecture.
- ADRs are never edited after acceptance (only superseded).

## Consequences

**Positive**:
- Each audience finds what they need at the right level of detail.
- AI assistants don't read every doc — `CLAUDE.md` is the contract.
- New contributors get the big picture from `docs/`, the action
  plan from `TODO.complete/`, and the rationale from ADRs.
- Bug reporters have a template (`BUGREPORT-*.md`).

**Negative**:
- **Same fact in multiple places**: e.g., "LZMA must round-trip
  via xz -d" appears in CLAUDE.md, TODO.complete, and the LZMA
  crate's README. Updating all is manual.
- **Stale docs are a tax**: if CLAUDE.md says "we have 11 crates"
  but the workspace has 17, the doc is wrong. Mitigated by
  quarterly doc audits (TODO 148 — code review sweep).
- **No single source of truth**: intentional, but means new
  contributors must learn the doc structure.

**Neutral**:
- This pattern is similar to how other multi-crate workspaces
  document themselves (e.g., `tokio-rs/tokio`).

## References

- [`CLAUDE.md`](../../CLAUDE.md) — operational contract.
- [`PLAN.md`](../../PLAN.md) — LZMA + ZSTD porting plan.
- [`TODO.complete/README.md`](../../TODO.complete/README.md) — task
  index.
- [`docs/`](../) — architecture + spec docs.
