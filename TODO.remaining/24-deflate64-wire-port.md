# 24 — Deflate64 wire-true port

- **Priority:** MEDIUM (real format support; no known consumer ask — LimniFS
  stores content-addressed data, not foreign zipx, but the crate
  ADVERTISES the format)
- **Depends on:** [23](23-deflate64-64k-probe.md) (oracle method)
- **Status:** pending 2026-09-05

## Goal

Make `omnizip-deflate64` decode (and then encode) the REAL PKWARE
Deflate64 wire format, validated bidirectionally against 7-Zip.

## What the real format adds over our current shape

- Length codes 257-272 (match lengths up to 65 538).
- 64 KB distances — the true distance-code layout that [22]'s probe
  was originally about (our table's `32769` entry and the Ruby's
  inconsistent encode side both suggest the port guessed).
- The header parse divergence (`literal table length exceeds buffer`)
  is the FIRST thing to diagnose: dump 7zz's first block header bits
  (BFINAL/BTYPE/HLIT/HDIST/HCLEN) with a small bit-reader and compare
  against our reader's expectation.

## Acceptance

- Decode every oracle member byte-identically (recipe in task 23,
  with content forcing >32 768 distances).
- Our encoder's archives extract byte-identically via `7zz x`.
- Existing self-consistency tests updated to the wire-true tables
  (round-trips will change bytes — one-time output change, gated like
  any format fix).
- Fuzz gate + 7zz interop added as a standing test when 7zz exists.

## Notes

- 7zz is the local oracle for BOTH directions (it writes and reads
  method 9).
- Estimated: a focused session — decoder first, encoder second.
