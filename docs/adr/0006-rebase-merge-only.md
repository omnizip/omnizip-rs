# ADR-0006: Rebase-merge only, never push to main

- **Status:** accepted
- **Date:** 2026-07-20
- **Deciders:** Ronald Tse

## Context

A prior release (early omnizip-rs) was corrupted by direct commits
to `main` that bypassed CI. Two commits landed without tests; one
shipped a regression that took a week to diagnose. The user
(Ronald) was forced to revert `main` and delete a published tag.

This violates the global rule: "NEVER commit to main, NEVER push
to main, NEVER merge to main locally." It needs enforcement at the
project level.

## Decision

**All changes go through PRs, rebase-merged.** No exceptions.

Operationalized by:

1. **GitHub branch protection** on `main`:
   - Require pull request before merging
   - Require status checks to pass before merging
   - Require branches to be up to date before merging
   - Dismiss stale pull request approvals when new commits are pushed
   - Restrict pushes that create matching branches
2. **Local convention**: branches named `{type}/{slug}` (e.g.,
   `feat/csv-gap-closure`, `fix/lzma-eopm`, `docs/adrs`).
3. **Merge strategy**: `gh pr merge --rebase --delete-branch`. This
   produces a linear history (no merge commits) and removes the
   feature branch.
4. **Tag policy**: tags are released by `release-plz` in CI, never
   locally. Local `git tag` commands are forbidden.

## Consequences

**Positive**:
- Every change has a paper trail: PR description, CI logs, reviewer
  comments, commit messages.
- Linear history is easy to bisect: `git log --oneline` shows every
  commit in order.
- No "surprise" commits to main; the user reviews before merge.
- Tags are released by CI in a reproducible environment, so the
  published artifact matches what was tested.

**Negative**:
- **Slower than direct push**: even a one-line typo fix requires
  branch → PR → CI → merge. ~10 minutes of overhead per change.
  Acceptable; the cost of an unwanted change is much higher.
- **Force-push needed for rebase**: if the branch has multiple
  commits and main moved, we rebase locally + force-push. The
  branch protection's "Allow force pushes for non-administrators"
  setting permits this on feature branches (never on main).
- **CI cost**: each PR runs the full test matrix (Linux, macOS,
  Windows × Debug, Release). ~$0.50 of GitHub Actions per PR.

**Neutral**:
- The user has explicitly authorized `gh pr merge --rebase` for the
  assistant's use; no additional confirmation per merge needed.

## References

- [Global rule (user's CLAUDE.md)](https://github.com/omnizip/omnizip-rs/blob/main/CLAUDE.md)
- [GitHub branch protection](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-branches-in-your-repository)
- [release-plz](https://release-plz.dev/)
