# omnizip-rs — Compliance Documentation

This directory documents every known divergence between the omnizip-rs
Rust implementation and:

- **RFC 8878** — the Zstandard Compression specification.
- **RFC 8724** — ZLIB Compressed Data Format (DEFLATE; not in scope yet).
- **The LZMA spec** — `xz-utils` / `tukaani-project/xz`.
- **The C reference implementations** — `facebook/zstd`, `tukaani-project/xz`.
- **The Ruby omnizip reference** — `omnizip/omnizip` (the porting basis).

When a Rust module intentionally deviates from any of these sources,
the deviation is documented here with:

1. What the spec / C / Ruby says.
2. What the Rust port does instead.
3. Why the deviation exists.
4. When (or whether) it will be reconciled.

## Index

### ZSTD

- [compliance-zstd-fse-table.md](compliance-zstd-fse-table.md) — FSE
  decode-table builder does not handle low-probability `-1` sentinel
  symbols. This is the current blocker for compressed-block decode.
- [compliance-zstd-bitstream-order.md](compliance-zstd-bitstream-order.md) —
  Reverse bitstream reader uses LSB-first within each byte (RFC-correct);
  the Ruby port reads MSB-first (Ruby bug).
- [compliance-zstd-offset-indexing.md](compliance-zstd-offset-indexing.md) —
  Offset symbols are 0-indexed in the FSE table; RFC text describes
  them as 1-indexed. The Rust port uses 0-indexed.
- [compliance-zstd-literals-size-format.md](compliance-zstd-literals-size-format.md) —
  Literals header size format follows the C reference
  (`byte0 >> 3` for 1-byte header), not the Ruby's `& 0x1F`.
- [compliance-zstd-huffman-fse-weights.md](compliance-zstd-huffman-fse-weights.md) —
  Huffman FSE-compressed weights reader is not yet ported. Compressed
  literals blocks return `Unsupported`.
- [compliance-zstd-checksum.md](compliance-zstd-checksum.md) — Frame
  content checksum is consumed but not verified. Real XXHash32 is not
  yet ported.

### LZMA

- [compliance-lzma-single-stream-only.md](compliance-lzma-single-stream-only.md) —
  Only the `.lzma` (LZMA-Alone) container is decoded. `.xz` and `.lz`
  containers, and LZMA2 multi-chunk, are not yet ported.
- [compliance-lzma-growing-output.md](compliance-lzma-growing-output.md) —
  The decoder uses a growing `Vec<u8>` for output rather than the Ruby's
  pre-allocated circular buffer with `LZ_DICT_INIT_POS` offset.

## Bug reports

Ruby-side bugs discovered during porting are filed at
`../omnizip/BUGREPORT.{01..10}-*.md` in the Ruby repository. The
compliance docs here reference those bug reports where relevant.
