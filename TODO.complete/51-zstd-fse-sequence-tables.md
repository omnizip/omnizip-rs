# Task 51: ZSTD — add FSE mode for sequences + FSE Huffman weights

## Status: pending
## Priority: P1

## Problem

The ZSTD encoder only uses Predefined tables for LL/ML/OF sequences.
Custom FSE tables would improve compression ratio. The decoder also
needs MODE_FSE support for reading custom tables.

## Plan

- Wire `fse::encoder::compress` into `encoder/sequences.rs` to build
  custom FSE tables when Predefined isn't optimal.
- Implement mode selection (Predefined/RLE/FSE/Repeat) per the C
  reference `ZSTD_selectEncodingType`.
- Enable FSE-compressed Huffman weights in `huffman/weights.rs`.

## Files

- `omnizip-zstd/src/encoder/sequences.rs`
- `omnizip-zstd/src/sequences.rs` (decoder: add MODE_FSE)
- `omnizip-zstd/src/huffman/weights.rs`
