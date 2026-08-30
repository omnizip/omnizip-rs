# Task 08: Architecture — emission module extraction

## Status: done (2026-08-30)

## What moved

`omnizip-brotli/src/encoder/emission.rs` (1,979 lines) now holds the
entire metablock emission layer, extracted verbatim from
`from_spec_encoder.rs` (8,855 → 6,928 lines, −1,927):

- `emit_metablock_from_commands` (1,587 lines — headers, context
  maps, block splitting, tree building, symbol emission)
- `encode_huffman_chunk_body` / `encode_huffman_chunk_into`
- `write_huffman_table` (RFC 7932 §9.5 complex form + RLE)
- `append_writer`

Boundary rule (MECE): everything that writes a metablock's bits is in
`emission`; parse, cost models, and quality routing stay in
`from_spec_encoder`. Call sites are stable via a re-import under the
same names; stayers referenced from emission got `pub(crate)`;
`env_flag!` is re-exported from its single definition.

## Verification

- Byte-identical output across 12 cells: {rustsrc.txt, csv-real.csv,
  fits-synthetic.fit} × {q1, q5, q9, q11}, FNV-1a hashed before and
  after the move — all equal
- 100 brotli tests + integration suite green; cargo fmt clean

## Acceptance

- [x] from_spec_encoder.rs reduced by ~2,000 lines (−1,927)
- [x] All outputs byte-identical (12-cell hash gate)
- [x] All tests pass
