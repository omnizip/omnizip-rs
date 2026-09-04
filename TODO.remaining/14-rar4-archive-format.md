# 14 — RAR4 archive format: verification + closure

- **Priority:** MEDIUM (verification only)
- **Depends on:** nothing
- **Estimated effort:** 0.5 day
- **Status:** done 2026-09-04

## Verification results (2026-09-04)

- `cargo test -p omnizip-rar`: 7 unit + 8 corpus tests green.
- All four RAR4 volume sets decode fully via `Rar4Reader::open_volume_set`
  (multiple_files 6-part, single_file 3-part, uncompressed_files 10-part,
  part0001 4-part incl. 241 MB PPMd entry) — every entry length-correct.
- GAP FOUND AND FIXED: `ozip x/t/l` opened RAR archives from bytes only,
  so volume sets failed with "entry spans volumes beyond this set".
  `open_archive` now detects `.partNN.rar` naming and `name.rar` +
  `name.rNN` siblings, concatenates parts (same pattern as the existing
  `.001` handling), and `scan_volume_set` is exported from
  `omnizip_rar`. RAR4 volume sets added to CI
  (`multivolume_sets_decode_fully`) — they had no committed coverage
  despite being validated during development.

## Correction of record (2026-09-04)

This task was opened on a stale memory-index line claiming "RAR4
remaining". That is WRONG. RAR4 shipped complete in the 0.21.x wave:

- Pure-Rust LZSS unpack versions 15/20/26/29, PPMd var H/I (incl. the
  241 MB `ppmd_lzss_conversion` fixture), VM standard filters
  (E8 window constant fixed), four multivolume sets via
  `open_volume_set`.
- RAR3 entry AES + header-encrypted (`-hp`) + solid archives all
  decode byte-identical to `/tmp/unrar` (passwords from libarchive
  fixture history).
- Corrupt PPMd UAF fixtures return structured errors, no panics.
- `ozip -p` wires RAR4/RAR5 passwords (0.21.7).

## What remains

A closure verification pass, nothing more:

1. `cargo test -p omnizip-rar` green (debug + release,
   `RUST_MIN_STACK` as needed).
2. Extraction smoke over the libarchive RAR4 corpus (146-file set
   aligned with 7zz during the rar5 work) plus the encrypted fixtures.
3. Confirm `ozip x` handles a real-world RAR4 volume set end-to-end.
4. Update the stale `containers-progress` memory index line.

## Acceptance

- All of the above green; task marked done with evidence in this
  file; no code changes expected (any failure found is a bug report +
  fix, not new-format work).
