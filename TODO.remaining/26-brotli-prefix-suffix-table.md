# 26 — brotli PREFIX_SUFFIX table one-byte desync

- **Priority:** HIGH (reference-stream decoder correctness)
- **Status:** done 2026-09-06 — fixed, validated, shipped

## Symptom

Reference q11 brotli streams (brotli crate, quality 11) failed to
decode with `static dictionary not supported` — the sweep's last
>1.05x cell (`rfc.txt` brotli q11 ≈ 1.10x) was attributed to a
parse-shape residual (task 04/18 family). It was not a ratio gap at
all: our decoder could not read reference output that uses
transformed static-dictionary words.

## Root cause

`omnizip-brotli/src/dictionary.rs` `PREFIX_SUFFIX` — the RFC 7932
Appendix A length-prefixed string table — declared the 8-byte string
`" of the "` with length prefix `\x10` (16) instead of `\x08`. Every
entry after index 2 was desynced by one byte against the canonical
table (`brotli-decompressor-5.0.3/src/transform.rs`).

Why it survived:

- Self round-trips passed — our encoder never emitted the affected
  transforms on our corpus, so encode+decode agreed on the wrong table.
- `PREFIX_SUFFIX_MAP` was already correct (it matched the TRUE
  lengths), so slot 0–2 lookups (used by most tests) worked.

## Fix

One byte: `\x10` → `\x08` in the entry-2 length prefix
(dictionary.rs:72). The full 47-entry table now diffs clean against
upstream; transform 73 = `" the " + word + " of the "` resolves
exactly as RFC Appendix B specifies.

## Validation

- Full table diff vs brotli-decompressor 5.0.3: all 47 entries match.
- `REF q11 rfc → ours`: BYTE-IDENTICAL (was: decode error).
- ours→ours round-trips, corpus × {q5..q11}: BYTE-IDENTICAL (6 files).
- Encoder output stability: fresh q11 encode of rfc.txt is
  byte-identical to the pre-fix encode — decoder-only fix, no
  baseline churn.
- Gates: `cargo test -p omnizip-brotli` (debug+release), fuzz-smoke,
  determinism, fmt, typos, clippy (CI style) — all green.

## Evidence

- Sweep cell `rfc.txt` brotli q11: reference stream now decodes; the
  1.10x cell was "reference produces transforms we cannot read",
  closing the last >1.05x brotli cell explanation.
- Post-tiering 70-cell sweep: no remaining brotli cell >1.05x.
