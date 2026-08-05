# TODO 154: Deflate64 from-spec implementation

## Problem

`omnizip-deflate64` (1308 LOC) implements an enhanced DEFLATE
variant (64 KB window, larger distances, extended Huffman codes).
The current implementation is partly in-house; verify no external
deps and finish any missing pieces.

## Scope

DEFLATE64 vs DEFLATE:
- 64 KB sliding window (vs 32 KB).
- Larger max match length (65538 vs 258).
- Extended distance codes (added 30-31).
- Used in some ZIP variants.

## Acceptance criteria

- [ ] No external deps in `Cargo.toml`.
- [ ] Round-trip parity with 7-Zip's DEFLATE64 output.
- [ ] Differential test against the reference C tool.

## Priority

P2.
