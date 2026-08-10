# 262 — Unified Codec Options Builder

- **Priority:** P3 (DX: consistent per-codec configuration)
- **Crate:** workspace
- **Depends on:** [248](248-codec-profile-enum.md)
- **Estimated effort:** 2 days

## Problem

Per-codec options structs exist but each follows its own pattern:

- `BrotliOptions { quality: u8, window_bits: u8 }`
- `ZstdOptions { ... }`
- `LzmaOptions { ... }`

Plus there are LzmaOptions, FlacOptions, etc. Some impl Default,
some don't. Some take CompressionLevel, some take u8. Callers
learning one codec's options don't get transferable knowledge.

## Design

### Generic options builder

```rust
/// Codec-agnostic options builder. Callers construct via
/// `Options::for_codec::<BrotliCodec>()` or build manually.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub level: Option<CompressionLevel>,
    pub profile: Option<Profile>,
    pub content_hint: Option<ContentType>,
    pub window_log: Option<u32>,  // LZMA / ZSTD
    pub chain_log: Option<u32>,   // ZSTD
    pub dictionary: Option<Vec<u8>>,  // ZSTD dict support
}

impl Options {
    pub fn level(mut self, level: u8) -> Self { ... }
    pub fn profile(mut self, profile: Profile) -> Self { ... }
    pub fn build(self) -> Self { ... }
}
```

### Per-codec interpretation

```rust
pub trait Codec: Send + Sync {
    // ... existing methods ...

    /// Compress with structured [`Options`].
    fn compress_with_options(
        &self,
        plaintext: &[u8],
        options: &Options,
    ) -> Result<Vec<u8>, OmnizipError> {
        // Default: translate to level + profile.
        let level = options.level.unwrap_or_else(|| ...);
        self.compress(plaintext, level)
    }
}
```

### Migration

Existing per-codec options structs remain for backward compat but
delegate to the shared `Options`:

```rust
impl BrotliOptions {
    pub fn to_options(&self) -> Options { ... }
}
```

## Acceptance criteria

- [ ] `Options` builder in omnizip-codecs.
- [ ] Default `compress_with_options` impl on Codec trait.
- [ ] Brotli, ZSTD, LZMA override to honor per-codec fields.
- [ ] Documentation shows migration path.
- [ ] Existing per-codec options deprecated but not removed.

## Why this matters

Today callers learn a new API per codec. With unified Options,
the common knobs (level, profile, content hint) work everywhere.
Per-codec knobs (window_log, chain_log) live in the same struct
but only the relevant codec reads them.
