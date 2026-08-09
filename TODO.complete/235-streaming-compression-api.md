# 235 — Streaming Compression API

- **Priority:** P3 (enables real-time / pipe usage)
- **Crate:** `omnizip-codecs` (trait) + per-codec implementations
- **Depends on:** none
- **Estimated effort:** 3 days

## Goal

Add a streaming compression API for incremental processing of unbounded
inputs. Currently all codecs require the full input in memory before
compression. A streaming API enables:
- Compressing files larger than RAM
- Pipe-based workflows (stdin → compress → stdout)
- Real-time compression of network streams

## Design

```rust
pub trait StreamingCodec: Codec {
    /// Create a streaming compressor instance.
    fn create_stream(
        &self,
        level: CompressionLevel,
    ) -> Result<Box<dyn StreamCompressor>, OmnizipError>;
}

pub trait StreamCompressor: Send {
    /// Feed input bytes. Returns compressed output produced so far.
    fn feed(&mut self, input: &[u8]) -> Result<Vec<u8>, OmnizipError>;

    /// Finalize the stream. Returns final compressed bytes.
    fn finish(&mut self) -> Result<Vec<u8>, OmnizipError>;
}
```

## Implementation per codec

Each codec implements `StreamCompressor` differently:
- **ZSTD**: Emit frame header on first feed, accumulate data into blocks,
  flush blocks on feed/finish.
- **Brotli**: Accumulate data into metablocks, emit on block boundary.
- **LZMA**: Use LZMA2 chunk streaming.
- **Others** (DEFLATE, LZ4, etc.): Wrap their native streaming APIs.

## Acceptance criteria

- [ ] `StreamingCodec` trait defined in `omnizip-codecs`
- [ ] ZSTD streaming compressor implemented and tested
- [ ] Brotli streaming compressor implemented and tested
- [ ] Round-trip: stream-compress → stream-decompress = identity
- [ ] Memory bounded regardless of input size
- [ ] Determinism: same input → same output (byte-identical)
