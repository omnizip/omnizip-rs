# 226 — ZSTD Version Bump for LDM Integration

- **Status:** DONE (omnizip-zstd 0.16.2+ published with LDM)
- **Priority:** P1 (blocking release)
- **Crate:** `omnizip-zstd`
- **Depends on:** [213](213-zstd-ldm.md)
- **Estimated effort:** 0.1 days

## Goal

Bump omnizip-zstd version to 0.16.3 to publish the LDM integration
(merged in PR #204). The LDM module existed at 0.16.2 but was not
wired into the block encoder until PR #204.

## Background

release-plz uses `command: release` which publishes based on
Cargo.toml version changes. Since the version wasn't bumped in PR #204,
release-plz sees 0.16.2 as already published and skips the release.

## Plan

1. Bump `omnizip-zstd/Cargo.toml` version from 0.16.2 to 0.16.3
2. Commit on a PR branch
3. Merge to main → release-plz publishes 0.16.3

## Acceptance criteria

- [ ] omnizip-zstd 0.16.3 published to crates.io
- [ ] `cargo add omnizip-zstd@0.16.3` includes LDM integration
