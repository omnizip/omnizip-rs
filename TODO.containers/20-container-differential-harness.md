# 20 — Container differential harness + oracles

- **Priority:** P0 (extend before the first format task ships)
- **Depends on:** [01](01-archive-core.md), existing `tests/differential`
- **Estimated effort:** 1–2 weeks
- **Crate:** `tests/differential` (extension)

## Goal

Extend the cross-language gate to containers, in oracle order:

1. **Ruby parity** — for each fixture under `omnizip/spec/fixtures/<format>/`:
   Ruby reads it → entry list + extracted bytes; Rust reads it → identical.
2. **Cross-tool oracles** (where installed; feature-gated so CI can tier them):
   `bsdtar` (tar/cpio/iso), `unzip`/`zip`, `7z`, `unrar` (rar4/5), `rpm2cpio`,
   `xar` — our reader's output must match theirs byte-exactly.
3. **Encoder round-trip through a foreign decoder** — we create, the oracle
   extracts, byte-exact vs the source tree; plus double-create determinism.

The Ruby runner gains `archive` mode (list/extract a fixture, print a
canonical manifest + per-entry SHA256) so JSON, not archives, crosses the
language boundary.

## Acceptance

- [ ] Harness runs Ruby-vs-Rust archive manifests on the full fixture corpus
- [ ] Oracle tier wired for at least tar/zip at merge; each format task adds
      its own oracle before it can be marked done
- [ ] Any divergence blocks release (same rule as codecs)
