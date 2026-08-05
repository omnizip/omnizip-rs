# TODO 116: DEFLATE dynamic-Huffman block encoder

## Problem

`omnizip-libdeflate/src/deflate_lz77.rs` currently emits only
**fixed-Huffman** blocks (BTYPE=1). For text/binary inputs the
**dynamic-Huffman** block (BTYPE=2) — where the encoder ships its own
optimised Huffman tables — gives 10-20% better ratio.

For the synthetic enwik-like bench text (64 KiB):

```
deflate-9: 23.1 MB/s  ratio=5.52×
```

The reference `gzip -9` produces ratio ≈ 6.5× on the same input — the
1× gap is entirely due to lack of dynamic Huffman.

## Proposed fix

Add a dynamic-Huffman block writer:

1. **Frequency counting**: tally symbol frequencies for literals
   (0-255), end-of-block (256), and length/distance codes.
2. **Huffman table construction**: package-merge with the DEFLATE
   constraint that no code length exceeds 15 bits. Already implemented
   in `omnizip-zstd/src/huffman/package_merge.rs` — DRY opportunity.
3. **Table encoding**: emit `HLIT/HDIST/HCLEN` header + code-length
   codes (also Huffman-coded, with a fixed 19-symbol alphabet and a
   permutation specified by the spec).
4. **Block emission**: write BTYPE=2, the table, then the
   Huffman-coded symbols.

The encoder picks BTYPE per block: dynamic, fixed, or stored —
whichever is smallest. Mirrors what `gzip` does.

## Acceptance criteria

- [ ] BTYPE=2 dynamic-Huffman block writer lands.
- [ ] Round-trips through `miniz_oxide::inflate::decompress_to_vec`
  and through `gzip -d`.
- [ ] Ratio improves 10-20% on text/binary inputs vs current.
- [ ] Encoder speed drops by < 30% (dynamic Huffman is more work but
  the per-block cost is amortised).

## Priority

P1 — significant ratio win, well-defined scope.

## Dependencies

- DRY with `omnizip-zstd/src/huffman/package_merge.rs` (TODO 114
  should land first so the package-merge is shared).
