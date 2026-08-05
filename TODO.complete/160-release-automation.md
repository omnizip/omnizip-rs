# TODO 160: Release automation (cargo-workspaces + tags)

## Problem

The current release process is manual:
1. Bump version in 3 Cargo.toml files.
2. Open a release PR.
3. Merge.
4. Run `publish.sh`.
5. (Sometimes) tag.

This is error-prone and easy to skip. Tags in particular have been
forgotten multiple times.

## Proposed fix

1. `cargo-workspaces` for batch version bump + publish.
2. `.github/workflows/release.yml` that:
   - Triggers on tags matching `v0.14.*`.
   - Runs `cargo publish` for each crate in dependency order.
   - Auto-generates release notes from commit log.
3. `release.toml` documents the per-crate release order.
4. Document the release runbook in `docs/release.md`.

## Acceptance criteria

- [ ] Tag push triggers automatic crate publishing.
- [ ] Release notes auto-generated.
- [ ] `docs/release.md` runbook lands.

## Priority

P2.

## Notes

The user's global rule: NEVER push tags without explicit approval.
This automation should request tag creation, not perform it.
