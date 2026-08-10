# 261 — Codec Capability Metadata

- **Priority:** P3 (DX: lets callers discover what a codec supports)
- **Crate:** `omnizip-codecs`
- **Depends on:** [251](251-codec-streaming-api.md) (streaming), [248](248-codec-profile-enum.md)
- **Estimated effort:** 1 day

## Problem

Callers have no programmatic way to discover a codec's capabilities.
Today the questions "does codec X support streaming?", "what's its
level range?", "does it have a fast binary mode?" are answered by
reading docs or source.

This matters for:
- Generic benchmarking tools that adapt to each codec.
- GUI/file-format choosers that filter codecs by capability.
- LimniFS picking the right codec for a given file type.

## Design

### Capability struct

```rust
/// Static capability metadata for a codec. Returned by
/// [`Codec::capabilities`] so callers can discover what a codec
/// supports without try/except.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    /// Min/max supported compression level.
    pub min_level: u8,
    pub max_level: u8,

    /// Whether the codec supports [`StreamingEncoder`]/[`StreamingDecoder`].
    pub streaming: bool,

    /// Whether the codec supports [`ParallelBatch`].
    pub parallel_batch: bool,

    /// Whether the codec uses a static dictionary (text-heavy codecs).
    pub has_static_dictionary: bool,

    /// Whether the codec is content-type-aware (uses [`ContentType`] hints).
    pub content_type_aware: bool,

    /// Typical best-case throughput in MB/s on modern hardware.
    /// Ballpark figure for caller-side planning.
    pub approx_throughput_mbps: u32,
}

impl Codec {
    pub fn capabilities(&self) -> Capabilities { ... }
}
```

### Per-codec impls

Each codec overrides `capabilities()` with its actual numbers:

```rust
// Brotli
fn capabilities(&self) -> Capabilities {
    Capabilities {
        min_level: 0, max_level: 11,
        streaming: true,
        parallel_batch: true,
        has_static_dictionary: true,
        content_type_aware: true,
        approx_throughput_mbps: 50,
    }
}
```

### Use cases

```rust
// Pick a codec for streaming use
let codec = registry.iter()
    .find(|c| c.capabilities().streaming && c.capabilities().min_level <= 5)
    .expect("no streaming codec");

// Filter codecs with dictionary
let text_codecs: Vec<_> = registry.iter()
    .filter(|c| c.capabilities().has_static_dictionary)
    .collect();
```

## Acceptance criteria

- [ ] `Capabilities` struct + `Codec::capabilities()` default.
- [ ] All 15 codecs override with correct values.
- [ ] Documentation shows examples of filtering by capability.
- [ ] Existing API unaffected (capabilities() is a new method).

## Why this matters

Today, generic codec tooling (benchmarks, format choosers) needs
hardcoded per-codec knowledge. Capabilities metadata externalizes
that knowledge so tooling can be data-driven. OCP: new codecs add
their own capabilities without modifying the tooling.
