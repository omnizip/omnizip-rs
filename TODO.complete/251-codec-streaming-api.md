# 251 — Codec Streaming API (Architectural: scalability)

- **Priority:** P2 (enables pipe usage, large-file processing)
- **Crate:** `omnizip-codecs` (trait) + per-codec impls
- **Depends on:** none
- **Estimated effort:** 5 days

## Problem

Current API is `compress(&[u8]) -> Result<Vec<u8>>`. This buffers
the entire input and output in memory. For LimniFS:

- 1 GiB file → 1 GiB input buffer + ~700 MB output buffer + temp
  for encoding state. Out of memory on small machines.
- Network pipes (HTTP request body → compress → response) must
  buffer the whole request before responding.
- Streaming use cases (backup tools, archive creation) require
  chunked processing.

## Design

### Streaming trait

```rust
/// Streaming compressor. Callers feed input in chunks, then call
/// `finish` to flush remaining state.
///
/// Implementations MUST be deterministic: feeding the same input
/// in different chunk sizes produces byte-identical output.
pub trait CompressStream: Send {
    /// Feed input bytes. Returns compressed output produced so far.
    /// May return empty Vec if internal state needs more input
    /// before emitting output.
    fn feed(&mut self, input: &[u8]) -> Result<Vec<u8>, OmnizipError>;

    /// Signal end of input. Returns final compressed bytes
    /// (trailer, checksums, last block, etc.).
    fn finish(&mut self) -> Result<Vec<u8>, OmnizipError>;

    /// Total bytes consumed so far.
    fn input_consumed(&self) -> u64;

    /// Total bytes produced so far.
    fn output_produced(&self) -> u64;
}

/// Streaming decompressor.
pub trait DecompressStream: Send {
    /// Feed compressed bytes. Returns decompressed output produced
    /// so far. Returns `Ok(None)` when stream is complete.
    fn feed(&mut self, input: &[u8]) -> Result<Option<Vec<u8>>, OmnizipError>;

    /// Total decompressed bytes produced.
    fn output_produced(&self) -> u64;
}
```

### Codec trait extension

```rust
pub trait Codec: Send + Sync {
    // ... existing methods ...

    /// Start a streaming compression session.
    fn compress_stream(&self, level: CompressionLevel)
        -> Result<Box<dyn CompressStream>, OmnizipError>;

    /// Start a streaming decompression session.
    fn decompress_stream(&self)
        -> Result<Box<dyn DecompressStream>, OmnizipError>;
}
```

### Chunking determinism

Critical invariant: chunk boundaries in the INPUT must not affect
the OUTPUT. Achieved by:

- Buffering until a complete "unit" (LZMA chunk, ZSTD block, brotli
  metablock) is available.
- Emitting complete units only.
- The last chunk before `finish()` flushes the partial unit.

Each codec's streamer has its own unit size:
- Brotli: metablock (up to 16 MiB)
- ZSTD: block (up to 128 KiB)
- LZMA: LZMA2 chunk (up to 64 KiB)
- LZ4: frame block (up to 64 KiB)

### Memory budget

Streamers expose a `memory_usage()` method returning peak memory:

```rust
pub trait CompressStream: Send {
    // ... existing methods ...

    /// Peak memory used by this streamer (input buffer + output
    /// buffer + intermediate state).
    fn memory_usage(&self) -> usize;
}
```

Codecs with multiple strategies (LZMA's fast vs. normal encoder)
can offer different streamers with different memory profiles.

## Per-codec implementation plan

| Codec | Difficulty | Notes |
|---|---|---|
| LZ4 | Easy | Already chunk-based; just expose the chunking. |
| Snappy | Easy | Already streaming. |
| DEFLATE | Medium | Block-based; need to flush blocks. |
| ZSTD | Medium | Frame format supports streaming natively. |
| Brotli | Medium | Metablock-based; need to choose metablock size. |
| LZMA | Hard | Range coder needs careful flush. |
| BZip2 | Hard | Block-based but block size depends on input. |
| PPMd | Hard | Model state persists across input; range coder flush. |

## Acceptance criteria

- [ ] `CompressStream` / `DecompressStream` traits in omnizip-codecs.
- [ ] LZ4, DEFLATE, ZSTD streamers (easiest 3).
- [ ] Round-trip with chunk sizes 1, 16, 256, 4096, 65536 produces
      byte-identical output as single-shot.
- [ ] Brotli, LZMA streamers (priority order).
- [ ] Memory budget ≤ 2× input chunk size for each codec.
- [ ] Example in `omnizip-bench/examples/stream_compress.rs`.

## Why this matters

Streaming API is what separates a compression LIBRARY from a
buffer-converting UTILITY. LimniFS scaling beyond in-memory files
requires this. The current `Vec<u8>` API is a 1 GiB ceiling on
practical use.
