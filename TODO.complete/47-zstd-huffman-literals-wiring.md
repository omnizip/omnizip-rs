# Task 47: ZSTD Huffman literals wiring

## Status: deferred — ZSTD encoder works (ratio competitive with zstd -1). These are ratio improvement TODOs, not correctness gaps.
## Depends on: 46 (FSE fix)
## Priority: P0

## Problem

`encoder/block.rs::encode_compressed_content` always emits Raw literals.
The Huffman encoder at `huffman/encoder.rs` exists but isn't wired in.

## Plan

- In `encode_compressed_content`, try both Raw and Huffman, pick smaller.
- On Huffman encode error, fall back to Raw.

## Acceptance

- High-entropy literals → Raw wins.
- Low-entropy (repeated) literals → Huffman wins.
- All block round-trip tests pass.
