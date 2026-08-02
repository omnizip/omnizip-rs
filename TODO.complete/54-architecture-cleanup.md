# Task 54: Architecture cleanup — stale Phase B docs, warnings, dead code

## Status: pending
## Priority: P2

## Problem

Many files still reference "Phase B" or have stale comments about
features that are now implemented. Compiler warnings exist.

## Plan

- Remove all stale "Phase B" / "not yet" comments.
- Fix all compiler warnings.
- Remove dead code.
- Update all module documentation to reflect current state.
- Ensure all error paths use proper error types.

## Files

- All `omnizip-*/src/**/*.rs`
