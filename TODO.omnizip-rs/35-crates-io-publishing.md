# 35 — crates.io publishing pipeline

- **Priority:** P2 (enables LimniFS integration via cargo)
- **Depends on:** [10](10-lzma-phase-a-decoder.md)
- **Estimated effort:** 1 day
- **Location:** `.github/workflows/release.yml`

## Goal

Publish every codec crate to crates.io on tag-driven releases. Consumers
(LimniFS, others) depend on the published versions, not git.

## Pipeline

```yaml
# .github/workflows/release.yml
on:
  push:
    tags: ['v*.*.*']

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo publish -p omnizip-codecs --token ${{ secrets.CRATES_IO_TOKEN }}
      - run: cargo publish -p omnizip-lzma   --token ${{ secrets.CRATES_IO_TOKEN }}
      - run: cargo publish -p omnizip-zstd   --token ${{ secrets.CRATES_IO_TOKEN }}
      - run: cargo publish -p omnizip-deflate --token ${{ secrets.CRATES_IO_TOKEN }}
      # ... per-crate, in dependency order
```

## Pre-publish checklist

For each crate, before `cargo publish`:

1. **Version bump**: workspace version bumped, crate version matches.
2. **Changelog**: `CHANGELOG.md` updated with `## [vX.Y.Z] - YYYY-MM-DD`
   section listing breaking changes, new features, bug fixes.
3. **README**: crate-level README.md is current.
4. **License**: `LICENSE-MIT`, `LICENSE-APACHE`, `LICENSE-NOTICE.md`
   included in the crate (via `include = [...]` in `Cargo.toml`).
5. **Metadata**: `description`, `keywords`, `categories`, `repository`,
   `homepage`, `documentation` all populated.
6. **clippy + fmt + doc**: all clean.
7. **Tests**: workspace tests green on linux + macOS.
8. **Differential**: cross-language gate green.

## Crate names

| Crate | crates.io name |
|---|---|
| `omnizip-codecs` | `omnizip-codecs` |
| `omnizip-lzma` | `omnizip-lzma` |
| `omnizip-zstd` | `omnizip-zstd` |
| `omnizip-deflate` | `omnizip-deflate` |
| `omnizip-bzip2` | `omnizip-bzip2` |
| `omnizip-ppmd` | `omnizip-ppmd` |
| `omnizip-filters` | `omnizip-filters` |
| `omnizip-snappy` | `omnizip-snappy` |

All under the `omnizip-*` namespace, owned by the `omnizip` crates.io team.

## Acceptance

- `cargo publish --dry-run -p omnizip-lzma` succeeds.
- Tagging `v0.1.0` triggers the workflow; crates appear on crates.io within
  5 minutes.
- Each crate's crates.io page shows: README, changelog, license links.
- Consumers can add `omnizip-lzma = "0.1"` to their Cargo.toml and use it.

## Implementation notes

- The CRATES_IO_TOKEN secret is a publish-scoped token; rotate quarterly.
- crates.io doesn't allow re-publishing the same version. Bump version
  even for "trivial" fixes.
- The first publish per crate is manual (crates.io requires email
  verification for new crate names). Subsequent publishes are automated.
