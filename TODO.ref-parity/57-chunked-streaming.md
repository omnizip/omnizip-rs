# Task 57 — chunked streaming: bounded-memory codec APIs

Status: open (part of task 51 phase 2; the biggest architectural
item — the determinism design below is the crux)

## Gap

Ruby `chunked/`: `reader.rb` / `writer.rb` (64 MiB default chunk)
+ `memory_manager.rb` (max-memory budget, `:disk` spillover
strategy, allocate/release accounting) — process inputs far larger
than RAM. Rust: every codec API is one-shot `&[u8] → Vec<u8>`;
input size is capped by available memory.

## Scope

In omnizip-codecs:

```rust
pub trait StreamingCompressor {
    fn push(&mut self, data: &[u8]) -> Result<(), OmnizipError>;
    fn finish(&mut self) -> Result<Vec<u8>, OmnizipError>;
}
pub trait StreamingDecompressor {
    fn push(&mut self, data: &[u8]) -> Result<Vec<u8>, OmnizipError>;
    fn finish(&mut self) -> Result<Vec<u8>, OmnizipError>;
}
```

Construction takes `chunk_size` (the FORMAT-level block/flush
granularity) EXPLICITLY. **Determinism rule (the whole design):**
the emitted block boundaries are a pure function of (input bytes,
input length, declared chunk_size) — implementations buffer
internally until a full declared chunk is assembled, so WHEN bytes
arrive via `push` never affects output. Two streaming runs with the
same total input and same chunk_size are byte-identical; a run
byte-differs from the one-shot API only in block granularity, never
in content.

Impl order (by natural flush granularity):
1. **zstd** — frames already block-structured; flush per chunk.
2. **deflate** — stored/dynamic block flush points.
3. **bzip2** — block-per-chunk.
4. **lzma/xz** — LZMA2 chunk flush (the 2 MiB uncompressed chunk
   cap already gives the seam).
5. **brotli** — metablock flush (metadata blocks); last if the
   flush semantics threaten ratio parity targets.

Plus `ChunkedFileWriter`/`ChunkedFileReader` in a small
`omnizip-chunked` crate (or archive-core) for the Ruby writer/
reader semantics: fixed 64 MiB chunks, memory_manager with a
declared budget and spillover-to-tempdir strategy.

## Acceptance

- Determinism: randomized push-size partitions (1B..1MiB random
  boundaries) of the same input → identical output, every codec,
  every level (fuzz-style test, seeded).
- Decodability: streaming outputs decode in the system CLIs
  (differential harness) and our one-shot decoders.
- Bounded memory: RSS ceiling test on a multi-GB synthetic input
  (CI-scale proxy: window + 2×chunk_size + ε).
- One-shot outputs unchanged: existing suites green untouched.
- Bounded-work: worst-case buffering analysis documented per codec
  (incompressible input = max metadata overhead per chunk).

## References

`../omnizip/lib/omnizip/chunked/{reader,writer,memory_manager}.rb`.
