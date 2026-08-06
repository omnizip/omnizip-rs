# 170: Brotli Decoder — Two-Tree Dispatch State Machine

## Priority: P1

## Status: pending

## Context

The pure-Rust Brotli encoder (`fast_encoder.rs`, vendored from upstream's
`compress_fragment_two_pass`) produces valid Brotli streams verified by
`brotli -d` across 11 fixtures. The decoder (`decoder.rs`) handles
uncompressed metablocks but NOT Huffman-coded metablocks from our encoder.

## Problem

`compress_fragment_two_pass` uses TWO independent Huffman trees:
- `depth[0..64]` for INSERT/COPY codes (stored as the rearranged 704-entry
  `cmd_depth_704` array)
- `depth[64..128]` for DISTANCE codes 64–127 (stored as a separate 64-entry
  tree)

Both trees use canonical Huffman codes starting from 0. When the encoder
writes a command, it picks `bits_128[code]` — the canonical code from the
appropriate tree. The decoder reads from a SINGLE 704-symbol table and uses
`kCmdLut[symbol]`.

The mismatch: canonical Huffman codes for `depth[C]` in tree_64 differ from
canonical codes for `cmd_depth_704[N]` in tree_704 (where N is the position
holding `depth[C]`). The rearrangement doesn't preserve canonical code
assignment.

## Approach

Implement a state machine that dispatches reads between the two trees:

1. Build `cmd_tree` from `depth[0..64]` (inverse-rearranged from
   `cmd_depth_704`). Build `dist_tree` from `depth[64..128]` (read directly
   as the separate distance table).

2. State machine:
   - `ExpectCmd`: read from `cmd_tree`.
     - INSERT (code < 24): emit literals → `ExpectDist`.
     - COPY_LAST_DIST (codes 24–37): copy from last dist → `ExpectCmd`.
     - COPY (codes 38–63): copy from last dist → check if next is dist_tree.
   - `ExpectDist`: read from `dist_tree`.
     - Code 0 (= cmd 64): marker, no-op → `ExpectCmd`.
     - Code 1–15: special short distance → `ExpectCmd`.
     - Code 16+: DISTANCE code → update ring buffer → `ExpectCmd`.

3. For ambiguous COPY codes (38–53, could be from EmitCopyLen or
   EmitCopyLenLastDistance), peek at the next symbol to determine whether
   a distance follows.

## Acceptance Criteria

- `brotli_round_trips_property_fixtures` passes (un-ignored)
- All 17 fixtures round-trip: encode → decode → compare
- `cargo test --workspace` — 0 failures, 0 ignored
