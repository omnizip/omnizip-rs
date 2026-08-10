# 269 — Per-Codec README Files

- **Priority:** P3 (documentation/onboarding)
- **Crate:** workspace
- **Depends on:** none
- **Estimated effort:** 1 day

## Problem

Each codec crate has a one-line description in Cargo.toml but no
README. Users discovering the crate on crates.io see only the
manifest. They have to read source to learn:

- What format does it implement?
- What levels does it support?
- What's the wire format?
- How does it differ from other implementations?

## Design

Each codec crate gets a `README.md` following a common template:

```markdown
# omnizip-<name>

Pure-Rust implementation of the <Name> compression format.

## Status

- Encoder: ✅ / 🔄 / ⏳
- Decoder: ✅ / 🔄 / ⏳
- Wire-format parity with <reference>: ✅ / partial / pending

## Quick start

    use omnizip_<name>::<Codec>;
    use omnizip_codecs::{Codec, CompressionLevel};
    let compressed = codec.compress(input, CompressionLevel::new(5))?;
    let decompressed = codec.decompress(&compressed, input.len() as u32)?;

## Levels

| Level | Speed | Ratio | Notes |
|-------|-------|-------|-------|
| 0     | ...   | ...   | ...   |
| ...   | ...   | ...   | ...   |

## Wire format

Implements RFC <XXXX> section by section. Tested against
<reference implementation>.

## Determinism

Byte-identical output across runs, machines, and Rust versions.

## License

Dual MIT OR Apache-2.0.
```

## Per-codec specifics

Each README documents the codec's unique aspects:

- **Brotli**: static dictionary, context modeling, metablocks
- **LZMA**: range coder, LZMA2 chunks, XZ container
- **ZSTD**: FSE entropy, frame format, dictionary support
- **LZ4**: frame format, HC vs. Fast modes
- **DEFLATE/libdeflate**: dynamic Huffman, zlib/gzip framing
- **Snappy**: Preamble + tag-byte format
- **BZip2**: BWT + MTF + RLE + Huffman
- **PPMd**: PPM context model
- **FLAC**: audio, LPC subframes
- **FSST**: string table compression
- **Rice++**: integer coding
- **BLOSC**: meta-compressor (wraps LZ4)
- **GLZA**: grammar-based
- **ZPAQ**: archive format with context mixing
- **Deflate64**: PKWARE enhanced DEFLATE

## Acceptance criteria

- [ ] All 15 codec crates have README.md following the template.
- [ ] Each README links to the format spec (RFC or vendor doc).
- [ ] crates.io publication picks up the README (verified via
      `cargo publish --dry-run`).
- [ ] Cross-links: omnizip-codecs's README links to each codec.

## Why this matters

README is the front door for crates.io discovery. Without one, the
crate looks unmaintained. With one, users can quickly understand
what they're getting. Especially important for a "pure-Rust port"
claim — users need to see the wire-format parity status.
