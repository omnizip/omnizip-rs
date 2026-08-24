# 21 — Extraction security hardening

- **Priority:** P0 (ships inside [01](01-archive-core.md); tracked separately for audit visibility)
- **Depends on:** [01](01-archive-core.md)
- **Estimated effort:** 1 week + continuous corpus
- **Crate:** `omnizip-archive-core/src/security.rs`

## Threat model (untrusted archives — the Excavate lesson)

| Threat | Rule |
|---|---|
| Path traversal ("zip-slip", `../`, `..\`) | every entry path normalized + must resolve inside the destination; reject otherwise |
| Absolute paths (`/etc/passwd`) | strip-leading-slash option, default reject |
| Symlink escapes | symlink targets validated inside destination; no following out |
| Hardlink escapes | same target validation |
| Decompression bombs | cumulative output cap (default 1 TiB or 1000× archive size) + entry count cap |
| High-permission bits (setuid/setgid) | never restored by default |
| Windows reserved names/ADS (`CON`, `:stream`) | rejected on Windows targets |
| Duplicate entry names | allowed on read; never overwrite via `..` tricks |

Defaults are safe; power users opt out per-flag (`--unsafe-paths` etc.),
never globally.

## Acceptance

- [ ] Unit corpus: every row above has a crafted malicious archive that is
      rejected (checked into `tests/security/fixtures/`)
- [ ] Fuzz targets for each format's parser (extend the existing cargo-fuzz
      setup) with the security invariants as crash conditions
- [ ] Documented flags table in the CLI `--help`
