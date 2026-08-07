# 184: Streaming API

## Priority: P2 (feature completeness)

## Status: pending

## Context

All codecs currently operate on full buffers (`compress(&[u8]) →
Vec<u8>`). LimniFS and other consumers need incremental encode/decode
for large files that don't fit in memory.

## Design (OCP)

Add a `StreamingCodec` trait that extends `Codec`:

```rust
pub trait StreamingEncoder {
    type Error;
    fn write(&mut self, input: &[u8]) -> Result<(), Self::Error>;
    fn finish(self) -> Result<Vec<u8>, Self::Error>;
}

pub trait StreamingDecoder {
    type Error;
    fn write(&mut self, input: &[u8]) -> Result<Vec<u8>, Self::Error>;
    fn finish(self) -> Result<Vec<u8>, Self::Error>;
}
```

Each codec implements these independently. The existing `Codec` trait
is unchanged (OCP — open for extension, closed for modification).

## Implementation order

1. LZMA (has `encode_chunk_inplace` infrastructure)
2. ZSTD (frame already supports multi-block)
3. DEFLATE / Brotli / others

## Files

- New: `omnizip-codecs/src/streaming.rs` — trait definitions
- Per-codec: `streaming.rs` modules

## Acceptance criteria

- [ ] `StreamingEncoder` + `StreamingDecoder` traits in omnizip-codecs
- [ ] LZMA implements both
- [ ] ZSTD implements both
- [ ] Round-trip: write in 1KB chunks, finish, decode in 1KB chunks
- [ ] Output byte-identical to one-shot `compress`/`decompress`
- [ ] All existing tests still pass
