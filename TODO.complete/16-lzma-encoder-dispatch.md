# 16 — LZMA encoder dispatch

**Status**: ❌ Pending. Depends on [13], [14], [15].

## Goal

Replace the `lzma2_compress` stub in `omnizip-lzma/src/lib.rs:139`
with a real implementation. Wire the encoder into `LzmaCodec::compress`.

## API

```rust
/// Compress `plaintext` with LZMA2 in an XZ container at the given level.
pub fn xz_compress(plaintext: &[u8], level: LzmaLevel) -> Result<Vec<u8>, LzmaError>;

/// Compress `plaintext` as a single-member Lzip file at the given level.
pub fn lzip_compress(plaintext: &[u8]) -> Result<Vec<u8>, LzmaError>;

/// Compress `plaintext` as a raw LZMA-Alone stream.
pub fn lzma_alone_compress(plaintext: &[u8], lc: u32, lp: u32, pb: u32, dict_size: u32)
    -> Result<Vec<u8>, LzmaError>;
```

## Codec dispatch

```rust
impl Codec for LzmaCodec {
    fn compress(&self, plaintext: &[u8], level: CompressionLevel)
        -> Result<Vec<u8>, OmnizipError>
    {
        let level = LzmaLevel::new(level.as_u8().min(9));
        xz_compress(plaintext, level)
            .map_err(|e| OmnizipError::EncodeFailed { ... })
    }
}
```

## Files

- `omnizip-lzma/src/lib.rs` — remove `LevelUnavailable` stub, add
  `xz_compress` / `lzip_compress` / `lzma_alone_compress`.
- `omnizip-lzma/src/codec.rs` — implement `Codec::compress` properly.

## Tests

- `compress(decompress(x)) == x` for all fixtures.
- Differential: encode via Rust + decode via `xz -d` oracle.

## Acceptance

- `limnifs-core` codec tests pass on LZMA.
