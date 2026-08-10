# 274 — Brotli Static Dictionary Decode Path Audit

- **Priority:** P1 (correctness — our decoder rejects vendored output
  that uses dict references)
- **Crate:** `omnizip-brotli`
- **Depends on:** [244](244-brotli-decoder-wire-format-bugs.md)
- **Estimated effort:** 2 days

## Problem

The Brotli benchmark shows vendored C encoder output is rejected by
our decoder with "invalid back-reference distance" on EVERY test
input. The error message points at distance computation.

Hypothesis: vendored C encoder uses static dictionary references
(distance > max_backward_distance), but our decoder's
`dictionary_lookup` path doesn't handle all 121 transforms correctly
or computes the address wrong.

## Design

### Step-by-step audit

1. Encode a 50-byte text input with `brotli -qf`.
2. Hex-dump the bytes; manually walk through the RFC 7932 spec.
3. Identify the first bit our decoder diverges from spec.
4. Fix the divergence.
5. Add the input as a regression test.

### Likely fixes

- `dictionary_lookup` may not handle the `npostfix > 0` case (we
  currently emit NPOSTFIX=0 always, but vendored uses NPOSTFIX>0).
- `decode_distance_from_code` may compute addresses wrong for short
  codes 4-15 (near-rep codes with delta).
- The metablock header's `npostfix`/`ndirect` parsing may have an
  off-by-one.

## Acceptance criteria

- [ ] `brotli -qf` output on all 17 brotli fixtures decodes correctly.
- [ ] `brotli -q 11` output on `english_text_100k` decodes correctly.
- [ ] Test fixtures added for each previously-failing input.
- [ ] The brotli_benchmark `DECODE-FAIL` lines replaced with actual
      ratio comparisons.

## Why this matters

Until this is fixed, our claim of "Brotli compatibility" is false.
Real-world Brotli streams (from browsers, `brotli` CLI, Go's
brotli crate) cannot be decoded by omnizip-brotli.
