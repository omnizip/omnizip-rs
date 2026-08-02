# 15 — XZ container encoder

**Status**: ❌ Pending. Depends on [14].

## Source

- `omnizip/lib/omnizip/algorithms/lzma/xz_encoder.rb` (420 LOC)
- `omnizip/lib/omnizip/algorithms/lzma/xz_encoder_fast.rb` (640 LOC) — fast encoder
- `omnizip/lib/omnizip/algorithms/lzma/optimal_encoder.rb` (138 LOC) — Phase C, deferred

## Architecture

```rust
pub fn xz_encode(input: &[u8], level: LzmaLevel) -> Vec<u8> {
    let mut out = Vec::new();
    write_stream_header(&mut out);
    write_block_header(&mut out, ...);
    let lzma2_payload = Lzma2Encoder::new(...).encode(input);
    out.extend_from_slice(&lzma2_payload);
    write_block_padding(&mut out);
    write_block_crc32(&mut out, &block_bytes);
    write_index(&mut out);
    write_stream_footer(&mut out);
    out
}
```

## Stream layout

```
Stream Header    12 bytes: magic + stream_flags + CRC32
Block N:
   Block Header  variable: header_size + filter_flags + padding + CRC32
   Block Data    variable: compressed payload
   Block Padding 0-3 bytes: align to 4 bytes
   Check         0/4/8/32 bytes: per stream_flags.check_type
Index:
   Index Indicator  1 byte: 0x00
   Number of Records VLI
   For each record: unpadded_size, uncompressed_size (VLIs)
   Index Padding
   Index CRC32
Stream Footer    12 bytes: CRC32 + backward_size + stream_flags + magic
```

## Filters

Per `xz_encoder_fast.rb`: support LZMA2 only at first (Phase B).
Phase C adds BCJ-x86 + Delta filters via `omnizip-filters`.

## Files

- `omnizip-lzma/src/encoder/xz.rs`
- Re-export from `encoder/mod.rs`

## Tests

- Round-trip via `xz_decompress` (already ported).
- Differential: encode via Rust + decode via `xz -d` → byte-identical
  to original input.
- Multi-block: input > 1 MiB produces ≥ 2 blocks.

## Acceptance

- `cargo test --test lzma_parity` encodes every fixture via Rust and
  decodes via `xz -d`, asserting byte-identical round-trip.
