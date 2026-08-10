# 248 — Codec Profile Enum (Architectural: OCP)

- **Status:** DONE (Profile + ProfileKind in omnizip-codecs; default
  level hooks overridden in Brotli, ZSTD, LZMA, LZ4 Fast + HC)
- **Priority:** P2 (architectural quality — replaces ad-hoc u8 levels)
- **Crate:** `omnizip-codecs` (trait) + per-codec mappings
- **Depends on:** none
- **Estimated effort:** 2 days

## Problem

`CompressionLevel` is a `u8` newtype wrapping a raw integer. Callers
must know each codec's range (Brotli 0-11, ZSTD 1-22, LZMA 0-9,
LZ4 1-12, etc.) and pick the right number for their use case.

This violates OCP: every new codec that adds levels forces callers
to learn a new range. There's no way to say "give me max ratio" or
"give me balanced" without knowing codec internals.

The current workaround is documentation: "level 9 means X for
codec Y". This doesn't scale.

## Design

### Profile enum

```rust
/// User-facing compression profile. Codecs translate this to their
/// internal level via a per-codec mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Maximum speed. Skips dictionary, context modeling, optimal
    /// parsing. Use for hot-path writes where ratio is secondary.
    Fast,
    /// Default. Reasonable ratio at acceptable speed. TheLimniFS
    /// "balanced" profile maps here.
    Balanced,
    /// Maximum ratio. Uses all features (dictionary, context modeling,
    /// optimal parser, multi-pass). Slowest.
    MaxRatio,
    /// Profile with content-type hint. Lets the codec skip detection
    /// and tune parser parameters up front.
    ForContent { profile: Self, content: ContentType },
    /// Fully custom. Caller knows the codec and provides the raw level.
    Custom(u8),
}
```

### ContentType enum

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    /// English-like text, source code, CSV, JSON, XML, etc.
    Text,
    /// Object files, images, audio, encrypted blobs.
    Binary,
    /// Mix of text and binary (e.g., serialized data with embedded
    /// binary fields).
    Mixed,
    /// Auto-detect from input bytes.
    Auto,
}
```

### Codec trait extension

Add a method to `Codec`:

```rust
pub trait Codec: Send + Sync {
    // ... existing methods ...

    /// Translate a [`Profile`] to this codec's internal compression
    /// level. Default implementation maps to a sensible per-codec
    /// value; codecs with custom profiles override.
    fn profile_to_level(&self, profile: Profile) -> CompressionLevel {
        match profile {
            Profile::Fast => CompressionLevel::new(self.default_fast_level()),
            Profile::Balanced => CompressionLevel::new(self.default_balanced_level()),
            Profile::MaxRatio => CompressionLevel::new(self.default_max_ratio_level()),
            Profile::ForContent { profile, content: _ } =>
                self.profile_to_level(profile),
            Profile::Custom(level) => CompressionLevel::new(level),
        }
    }

    /// Convenience: compress using a [`Profile`] instead of a raw level.
    fn compress_with_profile(
        &self, plaintext: &[u8], profile: Profile,
    ) -> Result<Vec<u8>, OmnizipError> {
        let level = self.profile_to_level(profile);
        self.compress(plaintext, level)
    }

    // Hook methods for per-codec defaults. Default impls return
    // mid-range values; codecs override.
    fn default_fast_level(&self) -> u8 { 1 }
    fn default_balanced_level(&self) -> u8 { 6 }
    fn default_max_ratio_level(&self) -> u8 { 9 }
}
```

### Per-codec overrides

```rust
// omnizip-brotli
impl Codec for BrotliCodec {
    fn default_fast_level(&self) -> u8 { 1 }
    fn default_balanced_level(&self) -> u8 { 5 }
    fn default_max_ratio_level(&self) -> u8 { 11 }
    // ...
}

// omnizip-zstd
impl Codec for ZstdCodec {
    fn default_fast_level(&self) -> u8 { 1 }
    fn default_balanced_level(&self) -> u8 { 9 }
    fn default_max_ratio_level(&self) -> u8 { 19 }
    // ...
}
```

### Migration path

- Keep `compress(plaintext, level)` for backward compatibility.
- Add `compress_with_profile(plaintext, profile)` as the new
  recommended API.
- LimniFS and other callers migrate to `Profile::Balanced` instead
  of hard-coded `9`.

## Acceptance criteria

- [ ] `Profile` and `ContentType` enums added to omnizip-codecs.
- [ ] All 15 codec impls override the default level hooks.
- [ ] Documentation shows the new API as recommended; old API marked
      "low-level — use Profile instead".
- [ ] LimniFS sample code in README uses `Profile::Balanced`.
- [ ] `is_text_like()` moved to `ContentType::detect()` in shared
      module (DRY with brotli's local impl).

## Why this matters

Today, callers hard-code `CompressionLevel::new(9)` everywhere. When
we add a new codec with a different range (e.g., 0-3 only), every
caller needs to learn the new range. `Profile` makes the intent
first-class: "max ratio" means whatever the codec supports.

OCP: adding new profiles (e.g., `Profile::StreamingMaxBandwidth`)
doesn't break existing codecs — they get a sensible default via the
hook methods.
