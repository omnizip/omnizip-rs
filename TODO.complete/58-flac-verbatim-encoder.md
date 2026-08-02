# Task 58: FLAC VERBATIM frame encoder

## Status: pending
## Priority: P2

## Problem

The FLAC Codec trait produces raw PCM containers. A VERBATIM frame
encoder would produce real FLAC bitstreams (uncompressed within the
FLAC container, but with proper frame headers, CRC, STREAMINFO).

## Plan

- Write fLaC magic + STREAMINFO metadata block
- Write one VERBATIM frame per block (block size 4096)
- Frame header: sync code, block size, sample rate, channels, bps
- VERBATIM subframe: raw samples
- Frame footer: CRC-16
- Round-trip through the existing decoder

## Files

- `omnizip-flac/src/encoder.rs` (new)
- `omnizip-flac/src/lib.rs` — wire encoder into Codec trait
