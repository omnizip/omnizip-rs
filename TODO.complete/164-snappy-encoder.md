# TODO 164: Snappy encoder

## Problem

`omnizip-snappy` is decode-only today (107 LOC wrapping `snap`).
LimniFS reads Snappy streams but cannot write them. Some
interoperability scenarios (e.g., reading data produced by
Google's tools) require Snappy encode.

## Scope

Snappy format is simple:
- Varint-encoded uncompressed length preamble.
- Tagged literal/match stream (4 tag formats per tag byte).
- 64 KB sliding window.
- No entropy coding (raw LZ77).

Encode is straightforward; the existing decoder wraps `snap`. The
encoder is the missing half.

## Implementation plan

Either:
1. Use `snap::Encoder` (keeps the wrapper dependency).
2. Port the encoder from spec (closes the external-dep gap, pairs
   with TODO 131).

Recommend option 2 — TODO 131 is the pure-Rust port.

## Acceptance criteria

- [ ] `SnappyCodec::compress` lands.
- [ ] Round-trips through own decoder + reference `snappy` CLI.
- [ ] Throughput ≥ 200 MB/s on text.

## Priority

P1 — LimniFS-flagged.
