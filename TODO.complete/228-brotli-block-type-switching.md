# 228 — Brotli Block Type Switching Re-enablement

- **Priority:** P2 (moderate ratio win on diverse inputs)
- **Crate:** `omnizip-brotli`
- **Depends on:** [227](227-brotli-vendored-decoder-fix.md) (decoder must be
  correct first)
- **Estimated effort:** 2 days

## Goal

Re-enable block type switching for literal/insert/distance categories. The
infrastructure exists (`write_block_type_trees`, block-switch emission) but is
disabled due to wire-format mismatches in the full decoder path.

## Background

Block type switching allows the encoder to use different Huffman tables for
different regions of the input. For example, a CSV file with a header section
followed by data rows could use different literal tables for each region.

The C reference uses block type switching at quality 7-9 for inputs where it
improves ratio.

## Current state

- `write_block_type_trees` function: implemented, writes NBLTYPES code trees
- Block switch emission in encoding loop: implemented
- `use_block_switch = quality >= 7 && quality <= 9 && input.len() >= 256
  && !use_context`
- **Disabled** because the decoder's block switch handling has a wire-format
  mismatch

## Plan

1. Create a minimal test with NBLTYPESL=2 (2 literal block types)
2. Trace through encoder and decoder to find the bit-level mismatch
3. Fix the block switch code tree encoding or decoder's block-length reading
4. Re-enable block type switching at quality 7-9

## Acceptance criteria

- [ ] NBLTYPESL=2 round-trips correctly
- [ ] Block switch emission matches decoder's read order
- [ ] Ratio improvement >= 1% on diverse inputs at Q7-9
- [ ] No regression on uniform inputs
