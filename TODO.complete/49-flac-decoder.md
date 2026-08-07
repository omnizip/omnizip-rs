# Task 49: FLAC decoder port

## Status: deferred — FLAC codec works (86 tests pass, round-trip OK). These are feature improvements.
## Priority: P1

## Problem

Only PCM header parsers exist. Need a real FLAC decoder.

## Phase A (MVP): VERBATIM + CONSTANT

- `omnizip-flac/src/bitreader.rs` — MSB-first bit reader
- `omnizip-flac/src/crc.rs` — CRC-8 (0x07) + CRC-16 (0x8005)
- `omnizip-flac/src/streaminfo.rs` — STREAMINFO parser
- `omnizip-flac/src/frame.rs` — frame header/footer parser
- `omnizip-flac/src/subframe.rs` — VERBATIM + CONSTANT decoders
- `omnizip-flac/src/decoder.rs` — top-level decode()

## Phase B (Full): FIXED + LPC + Rice

- `omnizip-flac/src/rice.rs` — partitioned Rice residual
- `omnizip-flac/src/fixed.rs` — fixed predictor (orders 0-4)
- `omnizip-flac/src/lpc.rs` — LPC decoder

## Acceptance

- Decode reference `.flac` files produced by libFLAC.
- CRC-8/CRC-16 vectors from FLAC spec pass.
- Round-trip determinism.
