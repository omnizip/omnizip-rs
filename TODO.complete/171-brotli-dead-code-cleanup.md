# 171: Brotli Dead Code Cleanup

## Priority: P2 (code cleanliness)

## Status: DONE — 1496 LOC of dead code removed from compilation.

## What was cleaned (2026-08-07)

Removed five superseded modules from the brotli crate's compilation
(the `.rs` files are retained on disk as reference):

- `encoder.rs` (508 LOC) — old uncompressed-only encoder, superseded
  by `fast_encoder.rs` (q=0/1 two-pass) and `compress_fragment.rs`
  (q=2..6 one-pass).
- `huffman.rs` (381 LOC) — old Huffman encoder, only used by
  `encoder.rs`.
- `commands.rs` (286 LOC) — old command encoding, only used by
  `encoder.rs`.
- `encoder_error.rs` (26 LOC) — error type for old encoder.
- `huffman_lookup.rs` (295 LOC) — ported table-based Huffman decoder,
  unused (decoder uses flat 2^15 lookup in `decoder.rs`).

Also cleaned:
- Removed stale `#![allow(dead_code)]` from `compress_fragment.rs`.
- Updated module list in `lib.rs` with clear active/archived comments.
- Achieved zero warnings across the entire workspace.

## Active modules

- `lib.rs` — Codec trait, quality dispatch (q=0/1 → two-pass, q>=2 →
  compress_fragment).
- `decoder.rs` — trivial-layout fast path decoder.
- `decoder_full.rs` — full RFC 7932 decoder path.
- `dictionary.rs` — static dictionary + 121 transforms.
- `prefix.rs` — kCmdLut const fn + block-length prefix codes.
- `static_codes.rs` — UTF-8/SIGNED context lookup tables.
- `fast_encoder.rs` — q=0/1 two-pass encoder (vendored from upstream).
- `compress_fragment.rs` — q=2..6 one-pass encoder.
