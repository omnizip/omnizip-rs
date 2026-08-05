# TODO 119: Codec streaming API

## Problem

The current `Codec` trait is buffer-to-buffer:

```rust
pub trait Codec {
    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError>;
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError>;
}
```

This forces callers to materialise the entire input and output in
memory. For LimniFS payloads > 1 GiB this is a hard constraint.

## Proposed fix

Add an optional streaming trait:

```rust
pub trait StreamingCodec: Codec {
    fn compress_stream(
        &self,
        input: &mut dyn Read,
        output: &mut dyn Write,
        level: CompressionLevel,
    ) -> Result<u64, OmnizipError>;

    fn decompress_stream(
        &self,
        input: &mut dyn Read,
        output: &mut dyn Write,
    ) -> Result<u64, OmnizipError>;
}
```

Default implementations are provided that buffer-to-buffer via the
existing `compress`/`decompress`. Each codec overrides with a true
streaming implementation.

## Phased rollout

| Codec | Streaming support today | Effort |
|-------|------------------------|--------|
| LZMA (XZ) | partial — `xz` container supports it; LZMA2 chunks | small |
| ZSTD | yes — frame format is stream-native | small |
| DEFLATE / libdeflate | yes — `miniz_oxide::inflate::decompress_to_vec_zlib_with_limit` + streaming versions | small |
| Brotli | yes — upstream has `BrotliEncoderCompressStream` | small (until TODO 117 lands) |
| bzip2 | yes — block-by-block | medium |
| LZ4 | yes — frame format | small |
| FLAC | no — needs whole-frame metadata first | large |
| PPMd | no — context model is whole-input | large |
| ZPAQ | no — same | large |
| FSST/Rice++/GLZA/BLOSC | codec-dependent | varies |

## Acceptance criteria

- [ ] `StreamingCodec` trait lands in `omnizip-codecs`.
- [ ] At least LZMA, ZSTD, DEFLATE, LZ4 implement it.
- [ ] Differential parity preserved.
- [ ] Memory-bounded decompression: a 10 GiB compressed input can be
  decoded with < 64 MiB resident memory.

## Priority

P2 — important for LimniFS scaling, but not on the critical path for
the current perf gap closure.
