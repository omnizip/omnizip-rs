# 15 — CLI: containers in `ozip`

- **Priority:** P1 (after the first P0 formats land)
- **Depends on:** [18](18-cli-ozip-v0.md), [02](02-tar.md), [03](03-gzip-bzip2-files.md), [04](04-zip.md), then each format as it lands
- **Estimated effort:** 1–2 weeks per integration batch
- **Crate:** `ozip` (the CLI crate from [18](18-cli-ozip-v0.md))

## Goal

Archive operations over the format registry, mirroring the Ruby CLI's shape:

```
ozip c archive.zip files...      # create (format by extension or -f)
ozip x archive.tar.gz [-C dir]   # extract (auto-detect, compressed tar)
ozip t archive.7z                # list
ozip l archive.rar               # long listing
ozip --formats                   # what's registered, with rw/read flags
```

Progress reporting, selective extraction (`--include/--exclude/--filter`),
and `--deterministic` flag (the [17](17-determinism-normalization.md)
normalizations, default on).

## Ruby → Rust module map

| Ruby source | Rust module | Notes |
|---|---|---|
| `cli.rb` + `cli/` + `commands/` | `ozip/src/commands/` | argument shapes ported; UX may improve |
| `convenience.rb` one-liners | lib-facing helper | for the Rust API surface |

## Acceptance

- [ ] Smoke-level CLI parity with the Ruby CLI's feature matrix
- [ ] `ozip c` → reference tool verify → `ozip x` byte-exact, for every
      registered format, in CI
- [ ] Windows/macOS/Linux CI runners all green
