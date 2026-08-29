# Task 05: Container format validation

## Status: done (2026-08-29) — one interop bug found + fixed

## What ran

1. Full workspace suite: 1305 passed / 0 failed.
2. Container suites (archive-core, tar, cpio, zip, 7z, rar, iso, xar,
   rpm, ole, par2): 96 passed / 0 failed.
3. Bidirectional CLI spot-check via `ozip` on a 4-file scratch
   corpus (text, csv slice, font slice, small note):
   - **Write direction** — archives created by `ozip c`, verified
     and extracted by system tools: tar / tar.gz / tar.bz2 /
     tar.xz / tar.zst (bsdtar), zip (unzip), cpio (bsdcpio),
     7z (7zz). All extract byte-identical.
   - **Read direction** — archives created by bsdtar / gzip / zip /
     xz / bsdcpio(newc) / 7zz, extracted by `ozip x`. All extract
     byte-identical.

## Bug found: `./` root entry aborted extraction

bsdtar and GNU tar write a `./` directory entry as the first entry of
`tar -c -C dir .` archives. `SecurityPolicy::validate_entry` rejected
it ("entry name reduces to nothing"), so `extract_to` failed the
WHOLE archive — every mainstream macOS-created tar was unextractable.

Fix: new `SecurityPolicy::sanitize_entry` returning `Ok(None)` for
root-denoting names (`./`, `.`, `././.`); `extract_to` skips those
entries. `validate_entry` keeps its old rejecting behavior (public
face unchanged). Regression: `omnizip-tar` reader test with a
handcrafted bsdtar-style `./` header.

## Acceptance

- [x] cargo test --workspace: 0 failures
- [x] Container round-trips verified (unit suites + CLI spot-checks)
- [x] Bug fixed + regression-pinned
