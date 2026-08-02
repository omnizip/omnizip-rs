# 24 — ZSTD frame encoder

**Status**: ❌ Pending. Depends on [21], [23].

## Source

- `omnizip/lib/omnizip/algorithms/zstandard/encoder.rb` (228 LOC)

## Architecture

```rust
pub fn encode_frame(input: &[u8], level: ZstdLevel) -> Vec<u8> {
    let mut out = Vec::new();
    write_magic(&mut out);
    write_frame_header(&mut out, ...);
    // For Phase B (level Fastest/Fast): single Raw block (no compression).
    // For Phase B (level Default): single Compressed block.
    // For Phase C: multi-block with optimal block splitter.
    write_block(&mut out, ...);
    if header.has_checksum {
        write_xxhash32(&mut out, &output_bytes);
    }
    out
}
```

## Phase B minimal viable

For level=Fastest: emit a single Raw block with all input.
This produces valid ZSTD output that round-trips through any ZSTD
decoder. Not compressed, but valid.

For level=Default: emit a single Compressed block with raw literals
and no sequences (or with Huffman-compressed literals if input is large).

## Files

- `omnizip-zstd/src/encoder/frame.rs`
- `omnizip-zstd/src/encoder/block.rs`
- `omnizip-zstd/src/encoder/mod.rs`

## Tests

- Round-trip: `decompress(encode(x)) == x` for all fixtures.
- Differential: encode via Rust + decode via `zstd -d` oracle.
- Determinism: encode same input 10× → byte-identical output.

## Acceptance

- `limnifs-core` codec tests pass on ZSTD.
