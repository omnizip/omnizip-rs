# Task 50: ZSTD encoder — remove Huffman safety fallback

## Status: pending
## Priority: P0

## Problem

`encoder/block.rs::verify_huffman_literals` is a safety net that
prevents Huffman literals from being used when the table doesn't parse.
The Huffman encoder must work for ALL distributions without fallback.

## Plan

- Debug `huffman/encoder.rs::encode_literals` for edge cases.
- Fix the weight encoding (direct vs FSE-compressed).
- Fix the Huffman bitstream encoding (4-stream split, reverse bit order).
- Remove `verify_huffman_literals` from `block.rs`.

## Files

- `omnizip-zstd/src/huffman/encoder.rs`
- `omnizip-zstd/src/encoder/block.rs`
