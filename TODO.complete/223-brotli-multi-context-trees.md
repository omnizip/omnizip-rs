# 223 — Brotli Multi-Context Tree Expansion

- **Priority:** P3 (moderate ratio win on text)
- **Crate:** `omnizip-brotli`
- **Depends on:** [205](205-brotli-context-mode-selection.md)
- **Estimated effort:** 2 days

## Goal

Expand from 2 literal context trees to the full 64-context model
described in RFC 7932 §10.1. The current encoder splits literals into
just 2 trees (contexts 0-31 → tree 0, contexts 32-63 → tree 1). The C
reference uses up to 64 trees, each specialized for a different byte
context.

## Background

RFC 7932 context modes:
- LSB6 (0): `context = p1 & 0x3F` (64 contexts)
- MSB6 (1): `context = reverse(p1) >> 2` (64 contexts)
- UTF8 (2): context from lookup table based on p1, p2 (64 contexts)
- Signed (3): `context = (p1 + p2) >> 4` (64 contexts)

Each context maps to one of NTREES literal Huffman trees. With more
trees, each tree is specialized for a narrower byte distribution,
improving compression.

## Current state

- `ntrees_l = 2` when context modeling is active
- Context map: contexts 0-31 → tree 0, 32-63 → tree 1
- Only 2 Huffman trees are built, missing fine-grained context separation

## Plan

1. Increase ntrees_l to match the number of distinct contexts that
   benefit from separate trees (e.g., 4-8 trees for LSB6, more for UTF8)
2. Build a context map that assigns contexts to trees based on byte
   frequency similarity
3. Build per-tree Huffman trees
4. Verify the context map wire format is correct (already handled by
   `write_context_map`)

## Acceptance criteria

- [ ] ntrees_l >= 4 when input benefits from context separation
- [ ] Context map correctly assigns similar contexts to same tree
- [ ] Round-trip tests pass for all quality levels with context modeling
- [ ] Ratio improvement >= 1% on text fixtures
