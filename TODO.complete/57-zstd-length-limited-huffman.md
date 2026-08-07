# Task 57: ZSTD Huffman encoder — length-limited Huffman coding

## Status: deferred — ZSTD encoder works (ratio competitive with zstd -1). These are ratio improvement TODOs, not correctness gaps.
## Priority: P1

## Problem

The Huffman encoder uses simple clamping for code lengths > 11
(HUF_TABLELOG_MAX). This produces invalid Huffman tables for very
skewed distributions. A round-trip safety check catches these and
falls back to Raw literals.

## Plan

Implement the package-merge algorithm for length-limited Huffman
coding. This produces optimal code lengths that respect the max
length constraint while maintaining the Kraft inequality.

## Files

- `omnizip-zstd/src/huffman/encoder.rs` — replace `limit_lengths`
- `omnizip-zstd/src/encoder/block.rs` — remove safety check once fixed
