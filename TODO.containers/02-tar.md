# 02 — TAR

- **Priority:** P0 (first format; proves the archive-core layer)
- **Depends on:** [01](01-archive-core.md)
- **Estimated effort:** 1–2 weeks
- **Crate:** `omnizip-tar`

## Goal

Full TAR read/write with POSIX ustar + pax extensions and GNU long-name
support. The simplest real container — the place the trait pair and the
determinism rules ([17](17-determinism-normalization.md)) get proven.

## Ruby → Rust module map

| Ruby source | Rust module | LOC |
|---|---|---:|
| `formats/tar/reader.rb` | `reader.rs` | ~330 |
| `formats/tar/writer.rb` | `writer.rs` | ~335 |

(665 LOC total across the tar module.)

## Acceptance

- [ ] Round-trips every fixture under `omnizip/spec/fixtures/tar/`
- [x] `bsdtar -t` / `tar -tf` list our archives identically to the Ruby ones
- [ ] pax/GNU long names, modes, symlinks, dirs preserved
- [x] Deterministic: same tree + options ⇒ byte-identical `.tar` (mtime
      normalization flags per [17](17-determinism-normalization.md))
