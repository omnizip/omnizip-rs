# 17 — Deterministic archives (normalization)

- **Priority:** P0 (design now, enforce from the first format task)
- **Depends on:** [01](01-archive-core.md)
- **Estimated effort:** 1 week + per-format wiring
- **Crate:** every format crate

## Goal

The CLI's headline property: **the same input tree + the same options produce
a byte-identical archive across runs, machines, and Rust versions** — the
content-addressed-storage invariant lifted from codecs to containers.

Containers leak nondeterminism through places codecs don't have:

| Source | Rule |
|---|---|
| File mtimes | `--mtime=<fixed>` (default: SOURCE_DATE_EPOCH, else 1970-01-01) |
| uid/gid/uname/gname | normalized to 0/`root` unless `--preserve-owner` |
| Directory iteration order | always lexicographic by path (never readdir order) |
| Hash-map serialization | forbidden in header paths (same rule as codecs) |
| Creation/host tool fields | fixed string `ozip <version>` — version pinned per release |
| Permissions | source modes by default; `--mode-normalize` to 0644/0755 |
| Compression | existing codec determinism guarantees |

Normalization lives in `omnizip-archive-core` as `WriteOptions::deterministic()`;
format crates consume it, never invent their own.

## Acceptance

- [ ] `ozip c` twice on the same tree (different mtimes between runs) ⇒
      identical bytes, for tar + zip initially, every format thereafter
- [ ] Cross-machine check in CI (linux + macOS runners produce the same archive)
- [ ] Property test: shuffled directory walk still yields identical output
