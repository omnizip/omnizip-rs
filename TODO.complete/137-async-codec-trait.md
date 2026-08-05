# TODO 137: Async Codec trait

## Problem

The `Codec` trait is synchronous. LimniFS uses `tokio` for async I/O
and currently has to wrap every codec call in
`tokio::task::spawn_blocking` — extra thread-pool overhead per call.

## Proposed fix

Add an optional async trait in `omnizip-codecs`:

```rust
#[cfg(feature = "async")]
pub trait AsyncCodec: Codec {
    async fn compress_async(
        &self,
        plaintext: Vec<u8>,
        level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError>;

    async fn decompress_async(
        &self,
        compressed: Vec<u8>,
        expected_len: u32,
    ) -> Result<Vec<u8>, OmnizipError>;
}
```

Default implementation moves the work to `spawn_blocking`. Codecs
with native async support override per-call.

## Acceptance criteria

- [ ] `AsyncCodec` trait lands behind an `async` cargo feature.
- [ ] At least LZMA, ZSTD, DEFLATE implement it.
- [ ] LimniFS can drop `spawn_blocking` for codec calls.
- [ ] No regressions on the sync API.

## Priority

P2 — important for LimniFS ergonomics but not a correctness issue.
