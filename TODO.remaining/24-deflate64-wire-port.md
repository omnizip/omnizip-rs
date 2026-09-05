# 24 — Deflate64 wire-true port

- **Priority:** MEDIUM (real format support; no known consumer ask — LimniFS
  stores content-addressed data, not foreign zipx, but the crate
  ADVERTISES the format)
- **Depends on:** [23](23-deflate64-64k-probe.md) (oracle method)
- **Status:** decoder SHIPPED 2026-09-05; encoder remains

## Goal

Make `omnizip-deflate64` decode (and then encode) the REAL PKWARE
Deflate64 wire format, validated bidirectionally against 7-Zip.

## Diagnosis (2026-09-05, bit-level)

Not a table bug: `container.rs` is a CUSTOM Ruby-invented format —
8-byte header (lit_len/dist_len as BE u32) + serialized Huffman
tables + raw bitstream — and `decompress` ALWAYS parses it. There is
no real deflate bitstream layer wired in at all (`decoder.rs`'s
table-driven token decoder is reusable; the missing piece is the
wire side: BFINAL/BTYPE dispatch, stored blocks, fixed blocks,
dynamic-block code-length (HCLEN) decoding, then the d64 ranges).

7zz's stream for reference (first block, from the probe fixture):
`BFINAL=0 BTYPE=2 HLIT=259 HDIST=27 HCLEN=16` — plain deflate
dynamic header; the divergence is purely "we never parse this".

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

- Keep the custom container as a legacy path ONLY if something reads
  it (grep consumers first; the zip container reading method 9 would
  need migrating to the wire decoder).

- 7zz is the local oracle for BOTH directions (it writes and reads
  method 9).
- Estimated: a focused session — decoder first, encoder second.


## Decoder half — SHIPPED (PR pending)

`wire.rs` implements real RFC-1951 blocks with the d64 extensions,
tables verbatim from 7-Zip's `DeflateConst.h` (codes 30/31 = bases
32769/49153 + 14 extra bits; length code 285 = base 227 + 16 extra
bits). `decompress` routes legacy-container-first (strict length
validation) with wire fallback — foreign streams now decode.

Validated against 7zz on seven shapes: 70 KB distances (A+B+A),
mid-stream stored blocks (text+random+text — this found a
`byte_align` bit/byte-units bug), a 2.85 MB multi-block stream,
tiny, repetitive, and empty members — all byte-identical to `7zz x`.
A 7-Zip-produced wire vector is committed as a standing unit test.

Two quirks pinned during validation (both documented in-code):
- 7zz's writer pads unused code-length symbols with dummy lengths
  (Kraft-over-subscribed by strict accounting); its decoder builds
  with full=false. We mirror the tolerance — the canonical walk never
  reaches unreachable codes, and broken codes fail the 15-bit runaway.
- zlib rejects these streams at HDIST=32 (`too many length or
  distance symbols`) — that is the d64 extension, not corruption.

## Remaining: encoder half

Emit wire blocks (stored/fixed/dynamic + the d64 code ranges) in
place of the legacy container, acceptance = `7zz x` on our archives.
Until then our method-9 output stays legacy-readable-only (wire
streams from real tools decode fine).
