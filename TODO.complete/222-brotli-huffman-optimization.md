# 222 — Brotli Literal Huffman Optimization

- **Priority:** P3 (small ratio win, well-understood algorithm)
- **Crate:** `omnizip-brotli`
- **Depends on:** none
- **Estimated effort:** 1 day

## Goal

Ensure literal Huffman trees use length-limited code construction
(package-merge) for optimal bit-length assignment within the 15-bit
Brotli limit.

## Current state

The encoder builds Huffman trees but may not produce optimal code
lengths. The C reference uses a carefully tuned Huffman builder that
handles edge cases (single-symbol, all-equal-frequency, etc.).

## Plan

1. Verify the current Huffman builder produces optimal code lengths
2. If not, replace with package-merge algorithm (already implemented
   in omnizip-codecs)
3. Ensure all edge cases are handled:
   - Single symbol (0 bits per read, not 1)
   - Two symbols (1 bit each)
   - Skewed distributions
4. Add property tests comparing against a brute-force optimal builder

## Acceptance criteria

- [ ] Huffman code lengths match optimal (package-merge) within 0 bits
- [ ] Edge cases handled (single-symbol, all-equal)
- [ ] Property tests pass for random frequency distributions
- [ ] No ratio regression on existing fixtures
