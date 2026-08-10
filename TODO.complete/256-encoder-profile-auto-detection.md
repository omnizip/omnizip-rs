# 256 — Encoder Profile Auto-Detection

- **Status:** DONE (ContentType::detect() in omnizip-codecs; brotli's
  is_text_like() now delegates to it. Per-codec profile_to_level
  mapping via the Codec trait.)
- **Priority:** P2 (UX: lets callers say "I don't know")
- **Crate:** `omnizip-codecs` (auto-detect), per-codec tuning
- **Depends on:** [248](248-codec-profile-enum.md) (ContentType)
- **Estimated effort:** 2 days

## Problem

Callers must pass `CompressionLevel::new(N)` knowing the codec's
range and the input's content type. Today:

```rust
// LimniFS balanced profile
codec.compress(data, CompressionLevel::new(9))?;
```

This hard-codes 9. For Brotli that's near-max-ratio. For LZMA that's
high but not max. For LZ4 that's HC mode. The number "9" carries
no semantic meaning.

Worse, callers must detect content type themselves to tune parser
parameters. Brotli does `is_text_like(input)` internally; LZMA
and ZSTD each have their own checks.

## Design

### Content-type detection in shared module

Move `is_text_like` from brotli to `omnizip-codecs::content_type`:

```rust
pub enum ContentType {
    Text,
    Binary,
    Structured,  // CSV, JSON, XML — has structure but text bytes
    Mixed,
}

impl ContentType {
    /// Detect content type from input bytes.
    /// Cheap: O(N) single pass, no allocations.
    pub fn detect(input: &[u8]) -> Self {
        if input.is_empty() {
            return ContentType::Binary;
        }

        // Sample up to 4 KiB for speed.
        let sample = &input[..input.len().min(4096)];

        let mut printable = 0;
        let mut structural = 0;  // punctuation that suggests CSV/JSON
        let mut binary = 0;

        for &b in sample {
            match b {
                b'a'..=b'z' | b'A'..=b'Z' | b' ' | b'\n' | b'\r' | b'\t' => printable += 1,
                b',' | b'{' | b'}' | b'[' | b']' | b':' | b'<' | b'>' => structural += 1,
                0..=8 | 14..=31 | 127..=255 => binary += 1,
                _ => printable += 1,
            }
        }

        let total = sample.len() as u32;
        let printable_pct = (printable * 100) / total;
        let structural_pct = (structural * 100) / total;
        let binary_pct = (binary * 100) / total;

        if binary_pct > 10 {
            ContentType::Binary
        } else if structural_pct > 10 {
            ContentType::Structured
        } else if printable_pct > 80 {
            ContentType::Text
        } else {
            ContentType::Mixed
        }
    }

    /// Hint to codecs about expected patterns. Brotli's parser
    /// uses this to choose lazy2 + dictionary for Text, greedy
    /// for Binary.
    pub fn is_text_like(self) -> bool {
        matches!(self, ContentType::Text | ContentType::Structured)
    }
}
```

### Codec profile mapping

Each codec translates (Profile, ContentType) → internal config:

```rust
// Brotli
fn profile_to_config(profile: Profile, content: ContentType) -> BrotliConfig {
    let quality = match (profile, content) {
        (Profile::Fast, _) => 1,
        (Profile::Balanced, ContentType::Text) => 5,
        (Profile::Balanced, ContentType::Structured) => 6,
        (Profile::Balanced, ContentType::Binary) => 4,
        (Profile::Balanced, ContentType::Mixed) => 5,
        (Profile::MaxRatio, _) => 11,
        (Profile::Custom(q), _) => q,
    };
    BrotliConfig { quality, use_dict: content.is_text_like(), ... }
}
```

### Single-call API

```rust
// Old
let level = if is_text { 9 } else { 5 };
codec.compress(data, CompressionLevel::new(level))?;

// New
codec.compress_with_profile(data, Profile::Balanced)?;
// ContentType::detect() called internally
```

### Override hook

Callers who KNOW the content type can skip detection:

```rust
codec.compress_with_profile(
    data,
    Profile::ForContent {
        profile: Profile::Balanced,
        content: ContentType::Text,
    },
)?;
```

## Acceptance criteria

- [ ] `ContentType::detect()` in omnizip-codecs.
- [ ] `is_text_like` in brotli calls into shared detection.
- [ ] All 15 codecs implement `compress_with_profile`.
- [ ] LimniFS sample code uses `Profile::Balanced` (no hardcoded
      level).
- [ ] Auto-detection accuracy ≥ 95% on Silesia corpus (binary vs.
      text classification).

## Why this matters

The less callers need to know about codec internals, the more
usable the library. Auto-detection is the difference between
"compress this" and "compress this CSV at quality 5 with text
heuristics".
