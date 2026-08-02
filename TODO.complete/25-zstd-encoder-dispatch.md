# 25 — ZSTD encoder dispatch

**Status**: ❌ Pending. Depends on [24].

## Goal

Replace `compress` in `omnizip-zstd/src/lib.rs:115` (returns
`LevelUnavailable`) with real implementation.

## API

```rust
/// Compress `plaintext` at the given level.
pub fn compress(plaintext: &[u8], level: ZstdLevel) -> Result<Vec<u8>, ZstdError>;
```

## Codec dispatch

```rust
impl Codec for ZstdCodec {
    fn compress(&self, plaintext: &[u8], level: CompressionLevel)
        -> Result<Vec<u8>, OmnizipError>
    {
        let level = match level.as_u8() {
            0..=2 => ZstdLevel::Fastest,
            3..=9 => ZstdLevel::Fast,
            10..=16 => ZstdLevel::Default,
            17..=22 => ZstdLevel::Better,
            _ => ZstdLevel::Best,
        };
        crate::compress(plaintext, level).map_err(...)
    }
}
```

## Files

- `omnizip-zstd/src/lib.rs` — replace stub
- `omnizip-zstd/src/codec.rs` — implement `Codec::compress`

## Tests

- 100% pass on `tests/fixtures/zstd/` round-tripping.

## Acceptance

- `limnifs-core` ZSTD tests pass without `ruzstd` fallback.
