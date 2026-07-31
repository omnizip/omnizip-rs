# 00 — Architecture

- **Priority:** P0 (foundation — every other task depends on this)
- **Depends on:** —
- **Status:** design complete; implementation distributed across 01–03

## Workspace topology

```
omnizip-rs/
├── Cargo.toml                    # workspace root
├── omnizip-codecs/               # shared trait + registry + error types
│   └── src/
│       ├── lib.rs
│       ├── codec.rs              # Codec trait
│       ├── registry.rs           # CodecRegistry
│       ├── error.rs              # OmnizipError
│       └── level.rs              # CompressionLevel newtype
├── omnizip-lzma/                 # LZMA/LZMA2/XZ
│   └── src/
│       ├── lib.rs
│       ├── constants.rs
│       ├── state.rs
│       ├── bit_model.rs
│       ├── range_coder/
│       ├── match_finder.rs
│       ├── coder/                # literal/length/distance
│       ├── encoder/              # optimal, xz, xz_fast
│       ├── decoder/
│       ├── lzma2/
│       └── xz_container.rs
├── omnizip-zstd/                 # Zstandard
│   └── src/
│       ├── lib.rs
│       ├── constants.rs
│       ├── frame/
│       ├── fse/
│       ├── huffman/
│       ├── literals/
│       ├── sequences.rs
│       ├── encoder.rs
│       └── decoder.rs
├── omnizip-deflate/              # DEFLATE / DEFLATE64
├── omnizip-bzip2/                # bzip2
├── omnizip-ppmd/                 # PPMd7 / PPMd8
├── omnizip-filters/              # BCJ + delta
├── omnizip-snappy/               # NEW: Snappy
├── omnizip-bench/                # benchmark framework + corpus
└── tests/
    └── differential/             # cross-language harness
```

## Crate boundaries (MECE)

Each crate owns exactly one algorithm family. No crate reaches into another's
internals. Cross-crate communication is via:

- `omnizip-codecs::Codec` trait — the dispatch surface
- `omnizip-codecs::CodecRegistry` — runtime registration
- `omnizip-codecs::OmnizipError` — unified error type

### Why one crate per algorithm family

1. **Compile times.** A user who only needs LZMA doesn't compile ZSTD's FSE
   tables. Cargo features can gate, but crate boundaries are cleaner.
2. **Independent versioning.** LZMA can ship v0.2 while ZSTD is still v0.1.
3. **Independent semver breaks.** A breaking change to the match finder
   doesn't bump ZSTD's version.
4. **crates.io discoverability.** `omnizip-lzma` is a first-class crate, not
   a feature flag.

### Why `omnizip-codecs` is separate

The `Codec` trait, registry, and error types are shared across all codec
crates. Putting them in `omnizip-lzma` would create a cyclic dependency
(ZSTD depends on LZMA to get the trait). A separate `omnizip-codecs` crate
is the SSOT for the trait; every codec crate depends on it, never on each
other.

## Semantic newtypes

Per CAMPAIGN.md's "semantically-driven" principle:

- `CompressionLevel` — newtype wrapping `u8`, clamped per-codec
- `LzmaLevel(0..=9)` — LZMA-specific level
- `ZstdLevel` — enum (Fastest/Fast/Default/Better/Best) mapping to 1/3/6/12/22
- `DictionaryId(u32)` — ZSTD dictionary identifier
- `FilterId(u8)` — BCJ/delta filter identifier

No bare `u8` or `u32` crosses a crate boundary as a level or id.

## OCP via registries

Every variation point is a registry:

- `CodecRegistry` — compression codecs (LZMA, ZSTD, DEFLATE, ...)
- `FilterRegistry` — preprocessing filters (BCJ-x86, BCJ-ARM, delta, ...)
- `DictionaryRegistry` — trained ZSTD dictionaries (future)

Adding a codec/filter/dictionary = registering a new struct; no dispatch
code changes. See [01-codec-trait-registry.md](01-codec-trait-registry.md).

## Determinism

Every encoder MUST be deterministic: same input + same level + same
dictionary ⇒ byte-identical output across runs, across machines, across Rust
versions. Non-determinism (e.g. thread scheduling affecting block boundaries)
is a release blocker.

This is a hard requirement for LimniFS's content-addressed storage: two
encoders producing different bytes for the same input breaks `DropId =
BLAKE3(plaintext)` deduplication at the representation layer.

## Unsafe policy

`#![forbid(unsafe_code)]` workspace-wide. SIMD acceleration (task 32) is
gated behind a `simd` feature and uses `std::simd` (portable safe SIMD), not
raw `unsafe` intrinsics. If `std::simd` is insufficient for a hot path, the
`unsafe` block is reviewed line-by-line and documented with a safety
commentary.

## License

MIT OR Apache-2.0 workspace-wide. The Ruby ports inherit MIT compatibility
from omnizip's per-file Ribose headers. C reference algorithms consulted for
perf tuning carry their own licenses (0BSD for liblzma, BSD-3 for zstd) but
are not copied — only consulted.
