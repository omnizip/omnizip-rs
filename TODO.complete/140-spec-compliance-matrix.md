# TODO 140: Spec compliance matrix

## Problem

Each codec implements a different spec subset. There's no central
document saying which features are supported, which are skipped,
which are intentionally not implemented.

The result: callers can't tell from the docs whether a particular
ZSTD frame feature (e.g., skippable frames, dictionary IDs) is
supported.

## Proposed fix

`docs/spec-compliance.md` with a matrix:

| Codec | Spec ref | Feature | Status |
|-------|----------|---------|--------|
| LZMA | xz-utils spec | BCJ-x86 filter | ✅ |
| LZMA | xz-utils spec | BCJ-ARM filter | ⏳ |
| ZSTD | RFC ZSTD | Skippable frames | ⏳ |
| ZSTD | RFC ZSTD | Dictionary prefix | ✅ |
| DEFLATE | RFC 1951 | Dynamic Huffman | ✅ |
| DEFLATE | RFC 1951 | BTYPE=3 reserved | reject |
| ... | ... | ... | ... |

Each codec contributes its own matrix file under
`docs/spec/{lzma,zstd,...}.md`. The workspace-level index
aggregates them.

## Acceptance criteria

- [ ] `docs/spec-compliance.md` lands with the matrix.
- [ ] Per-codec files for each codec.
- [ ] Reviewer signoff that every "supported" claim matches
  implementation.

## Priority

P2 — important for documentation completeness, not on critical path.
