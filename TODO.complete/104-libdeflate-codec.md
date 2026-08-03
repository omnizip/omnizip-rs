# 104 — Libdeflate pure-Rust parity codec (codec id 0x000B)

**Priority:** Low — new codec
**Source:** LimniFS proposal `omnizip-proposals/libdeflate.md`
**Status:** ⏳ Pending — proposal landed; PR TBD

## Problem

`omnizip-codecs::CodecId::LIBDEFLATE = 0x000B` reserves a slot for a
libdeflate-compatible DEFLATE codec, but no `omnizip-libdeflate`
crate exists. LimniFS can't use this slot.

## What libdeflate is

Libdeflate (Eric Biggers, 2016+) is a faster DEFLATE implementation
than zlib:

| Codec          | Encode (MB/s) | Decode (MB/s) | Ratio |
|----------------|--------------:|--------------:|-------|
| zlib -6        |            50 |            250 | 100%  |
| libdeflate -6  |           200 |            600 | 100%  |
| libdeflate -12 |            30 |            600 |  98%  |

Decode speed is the headline: ~2.4× faster than zlib on the same
input. The codec is DEFLATE-compatible — same wire format, only the
encoder/decoder implementation differs.

## Why LimniFS cares

LimniFS doesn't need libdeflate for new images (we use ZSTD + Brotli
+ LZ4). But we receive plenty of legacy DEFLATE content:

- `.zip` archives (DEFLATE inside).
- `.jar` / `.war` files (Java's default).
- HTTP responses with `Content-Encoding: gzip`.
- `.git/objects` (zlib-compressed).

Today we decode via `omnizip-deflate` (wraps `miniz_oxide`).
`miniz_oxide` is correct but ~2× slower than libdeflate.

## Scope decision

**Decode-only is the priority.** LimniFS's primary use is decoding
legacy DEFLATE content. The encode side is a nice-to-have but not
blocking.

Land decode-only first; encode later if benchmarks justify.

## Wire format

Same as RFC 1951 DEFLATE. No new container — just a faster
implementation. Output is byte-compatible with `gzip -d` / `zlib
-d` / Python `zlib.decompress(data, -15)`.

## Implementation plan

### Phase 1 — Skeleton crate (1 day)

```
omnizip-libdeflate/
├── Cargo.toml
├── src/
│   ├── lib.rs          (LibdeflateCodec impl)
│   ├── huffman.rs      (faster Huffman decode)
│   ├── inflate.rs      (RFC 1951 decode)
│   └── bitreader.rs    (refill-heavy bit reader)
└── tests/
    └── parity.rs       (vs miniz_oxide on shared corpus)
```

### Phase 2 — Decode pipeline (8 days)

- Bit reader optimised for refill-heavy loops.
- Huffman: pre-built fast table (size 4096 entries, 2-level lookup
  for codes longer than 9 bits).
- Length/distance table lookups (RFC 1951 §3.2.5).
- Output buffer reuse, no per-byte allocation.

### Phase 3 — Encode pipeline (3 days, optional)

DEFLATE encoder using canonical Huffman + simple LZ77. Goal: ratio
within 5% of `zlib -6`. Speed not critical for encode.

## Acceptance criteria

### Decode (mandatory)

- [ ] Decode every fixture in the Calgary corpus.
- [ ] Decode gzip-encoded HTTP fixture sample (10 MB).
- [ ] Throughput ≥ 1.5× `omnizip-deflate` on Calgary decode.
- [ ] Round-trip: `inflate(deflate(x)) == x` for any DEFLATE encoder.

### Encode (optional, Phase 3)

- [ ] Encode ratio within 5% of `zlib -6` on Calgary.
- [ ] Encode throughput ≥ 100 MB/s on text input.

## Effort estimate

- Phase 1: 1 day
- Phase 2: 8 days
- Phase 3: 3 days (optional)
- **Total: 9-12 days** (decode-only); 12-15 days with encode.

## Out of scope

- libdeflate's specialised checksum (use shared `omnizip-codecs::checksum`).
- Multi-threaded inflate.

## Related

- omnizip-rs reserved `CodecId::LIBDEFLATE = 0x000B`
- LimniFS proposal `omnizip-proposals/libdeflate.md`
- libdeflate upstream: https://github.com/ebiggers/libdeflate
