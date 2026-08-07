# Task 53: FLAC — wire stream decoder to Codec trait

## Status: deferred — FLAC codec works (86 tests pass, round-trip OK). These are feature improvements.
## Priority: P1

## Problem

The FLAC Codec trait still uses raw PCM container, not real FLAC
bitstreams. The decoder modules exist (bitreader, crc, streaminfo,
frame, subframe, rice) but aren't wired through `decompress`.

## Plan

- Detect FLAC magic (`fLaC`) in `decompress`.
- Parse STREAMINFO metadata block.
- Decode all audio frames using the existing frame decoder.
- Convert decoded samples back to interleaved PCM bytes.
- Add a FLAC frame encoder (VERBATIM subframe for minimum viable).
- Replace raw-PCM Codec trait impl with real FLAC.

## Files

- `omnizip-flac/src/lib.rs`
- `omnizip-flac/src/decoder.rs` (new — top-level stream decoder)
- `omnizip-flac/src/encoder.rs` (new — VERBATIM frame encoder)
