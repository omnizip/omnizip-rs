# 01 — Codec trait + registry

- **Priority:** P0 (blocks every codec task 10–25)
- **Depends on:** [00](00-architecture.md)
- **Estimated effort:** half a day
- **Crate:** `omnizip-codecs`

## Goal

Define the `Codec` trait and `CodecRegistry` that every codec crate plugs
into. Adding a codec = implementing the trait + calling `register()`. No
dispatch code changes. Mirrors the `limnifs-core::codec` design (PR #110),
generalised for standalone use.

## Design

```rust
// omnizip-codecs/src/codec.rs
pub trait Codec: Send + Sync {
    fn id(&self) -> CodecId;
    fn name(&self) -> &'static str;
    fn compress(&self, plaintext: &[u8], level: CompressionLevel)
        -> Result<Vec<u8>, OmnizipError>;
    fn decompress(&self, compressed: &[u8], expected_len: u32)
        -> Result<Vec<u8>, OmnizipError>;
}

// omnizip-codecs/src/registry.rs
pub struct CodecRegistry { codecs: Vec<Box<dyn Codec>> }
impl CodecRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, codec: Box<dyn Codec>);  // panics on id collision
    pub fn compress(&self, id: CodecId, plaintext: &[u8], level: CompressionLevel)
        -> Result<Vec<u8>, OmnizipError>;
    pub fn decompress(&self, id: CodecId, compressed: &[u8], expected_len: u32)
        -> Result<Vec<u8>, OmnizipError>;
    pub fn default_pure_rust() -> Self;  // all pure-Rust codecs registered
}
```

### CodecId

Strong newtype wrapping `u16` (not `u8` — the portfolio will exceed 256
entries with all filter variants and newer algorithms).

```rust
pub struct CodecId(u16);
impl CodecId {
    pub const STORE: CodecId = CodecId(0x0000);
    pub const LZ4: CodecId = CodecId(0x0001);
    // ... assigned per-codec as they land
}
```

The codec id space is **not** the same as LimniFS's wire-format codec byte.
LimniFS maps its u8 wire ids to `omnizip-codecs::CodecId` at the integration
boundary (see [40-limnifs-integration.md](40-limnifs-integration.md)).

### CompressionLevel

```rust
pub struct CompressionLevel(u8);
impl CompressionLevel {
    pub const fn new(level: u8) -> Option<Self>;  // None if > max_for_codec
    pub const fn as_u8(self) -> u8;
}
```

Each codec clamps to its own range (LZMA 0–9, ZSTD 1–22, Brotli 0–11, etc.).
A codec receiving an out-of-range level returns
`OmnizipError::LevelOutOfRange`.

## Acceptance

- `omnizip-codecs` compiles with `#![forbid(unsafe_code)]` and clippy::pedantic.
- A unit test registers a custom `NoopCodec` with an arbitrary id and
  compresses/decompresses through the registry without touching dispatch code.
- A unit test confirms duplicate-id registration panics cleanly.
- `CodecRegistry::default_pure_rust()` is empty in this task; codecs register
  themselves in tasks 10–25.
- API documented with doc examples that compile as doctests.

## Implementation notes

- The default registry lives in a `static OnceLock<CodecRegistry>` so callers
  don't pay re-registration cost.
- `Codec` requires `Send + Sync` so the registry can be shared across rayon
  threads (LimniFS's writer is parallel).
- Look-up is `O(n)` where n is registered codec count (~10). If profiling
  shows it's hot (it won't be — compress dominates), switch to a
  `[Option<&dyn Codec>; 65536]` direct-indexed table.
