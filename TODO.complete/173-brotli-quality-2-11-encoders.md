# 173: Brotli — Q≥2 Encoder

## Priority: P4

## Status: skeleton ported — compiles but produces invalid output. Needs debugging.

## Context

The pure-Rust brotli encoder has two paths:

1. **fast_encoder.rs** (q=0/1) — vendored port of `compress_fragment_two_pass.c`.
   Produces valid brotli that all conformant decoders accept. **Active for all quality levels.**

2. **compress_fragment.rs** (q=2..6) — port of upstream `compress_fragment.c`.
   Skeleton committed but not yet producing valid output due to bugs in the
   command prefix code scatter pattern.

The q≥2 path emits combined INSERT+COPY commands via the 704-symbol alphabet,
achieving ~10-20% better ratio at higher CPU cost.

## What's done

- `compress_fragment.rs` (786 LOC) — full port of upstream functions:
  - Hash (8-byte), IsMatch (5-byte).
  - BuildAndStoreLiteralPrefixCode (histogram + Huffman).
  - BuildAndStoreCommandPrefixCode (128→704 scatter pattern).
  - Emit* functions (InsertLen, CopyLen, Distance, etc.).
  - Main match-finding loop with hash table.
  - Metablock management (header, merge, uncompressed fallback).
  - Entry point `compress()`.

## What's broken

The encoder produces output that both our decoder and `brotli -d` reject:
- Reference decoder: "corrupt input".
- Our decoder: "invalid static dictionary reference".

Likely root cause: the command prefix code scatter pattern in
`BuildAndStoreCommandPrefixCode` doesn't match the decoder's expectations.
The 128-symbol command alphabet needs to be correctly scattered into the
704-symbol space that the decoder's `kCmdLut` expects.

Debugging approach:
1. Compare byte-level output against upstream `brotli -q 2` for a known input.
2. Trace the scatter pattern step by step.
3. Verify the metablock header (13 bits of 0) encodes the correct layout.

## Why the existing encoder works

The q=0/1 two-pass encoder already produces valid, deterministic brotli.
For LimniFS (the consumer), determinism + round-trip integrity matter more
than max ratio. The q≥2 path is a ratio improvement that doesn't unblock
any consumer.

## Acceptance Criteria

- Round-trip via own decoder + `brotli -d` at quality 2..6.
- Ratio improvement over q=0/1 on text inputs.
- No nondeterminism: same input always produces identical bytes.
