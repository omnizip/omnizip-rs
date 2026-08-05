# omnizip-codecs

Shared trait + error types for the omnizip-rs codec workspace.

## What this crate provides

- [`Codec`](src/codec.rs) trait — the uniform interface every codec implements.
- [`CodecId`](src/codec.rs) — `u16` newtype with assigned constants
  (`LZMA`, `ZSTD`, `BROTLI`, etc.).
- [`CompressionLevel`](src/codec.rs) — `u8` newtype for level selection.
- [`OmnizipError`](src/error.rs) — unified error enum with codec-tagged
  variants (`EncodeFailed`, `DecodeFailed`, `LevelOutOfRange`,
  `LengthMismatch`, `Corrupt`).
- [`CodecRegistry`](src/registry.rs) — runtime registry for codec
  dispatch by id.
- [`HashChainMatchFinder`](src/matchfinder.rs) — reusable hash-chain
  LZ77 match finder shared by LZMA/LZ4/libdeflate/ZSTD.
- [`Filter`](src/filter.rs) trait — for preprocessing transforms
  (BCJ, delta, etc.).
- Shared modules: [`arith`], [`checksum`], [`hash`], [`xxhash`],
  [`matchfinder`].

## Usage

```rust
use omnizip_codecs::{Codec, CodecId, CodecRegistry, CompressionLevel};

let mut registry = CodecRegistry::new();
registry.register(Box::new(omnizip_zstd::ZstdCodec::new()));

let plaintext = b"hello world";
let compressed = registry
    .get(CodecId::ZSTD)
    .expect("zstd registered")
    .compress(plaintext, CompressionLevel::default())
    .expect("compress");
```

## Adding a new codec

1. Implement the `Codec` trait.
2. Allocate a `CodecId` constant in `src/codec.rs`.
3. Register via `CodecRegistry::register`.

The dispatch code never changes — this is the open/closed principle
applied to codec registration.

## Determinism

Every codec in the workspace must produce byte-identical output for
the same input + level across runs, machines, and Rust versions.
This is verified by `tests/determinism/`. The shared
[`HashChainMatchFinder`] is deterministic by construction (no
`HashSet` iteration, no thread-local state).

## License

MIT OR Apache-2.0.