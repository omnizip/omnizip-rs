# 32 — Yank 0.1.0 and publish 0.1.1

**Status**: ❌ Pending. Depends on bugs being fixed (01, 02).

## What

The published `omnizip-zstd 0.1.0` on crates.io has three bugs
(BUGREPORT-zstd-0.1.0.md). After fixing, publish 0.1.1 and yank 0.1.0.

## Steps

```bash
# 1. Bump version in omnizip-zstd/Cargo.toml
sed -i '' 's/version = "0.1.0"/version = "0.1.1"/' omnizip-zstd/Cargo.toml
# (also update workspace.dependencies)

# 2. Verify build + tests
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# 3. Dry-run publish
cargo publish --dry-run -p omnizip-zstd

# 4. Publish (user must run)
cargo publish -p omnizip-zstd

# 5. Yank 0.1.0
cargo yank --vers 0.1.0 -p omnizip-zstd
```

## Yank vs delete

`cargo yank` hides the version from new `cargo add` resolves but
keeps it downloadable for projects that already pin it. Crates.io
doesn't allow deletion (to preserve reproducibility for old builds).

## Acceptance

- `cargo search omnizip-zstd` shows 0.1.1 as latest.
- `cargo add omnizip-zstd` resolves to 0.1.1.
- Existing projects pinned to 0.1.0 still build.
