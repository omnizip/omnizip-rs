# omnizip-rs — pure-Rust compression codecs and containers

A 35-crate workspace of pure-Rust compression codecs and archive containers,
ported line-by-line from the [omnizip](https://github.com/omnizip/omnizip)
Ruby reference implementations (the C references — xz, zstd — were consulted
for performance tuning only, never as the porting basis). MIT OR Apache-2.0.

Every encoder is **byte-deterministic**: the same input + level produces
byte-identical output across runs, machines, and Rust versions. The
workspace is `#![forbid(unsafe_code)]` (sole exception: the raw-pointer shim
inside `omnizip-ffi`).

## Usage

### The `ozip` CLI

```sh
ozip c archive.tar.zst dir/          # create (deterministic by default)
ozip x archive.tar.zst               # extract
ozip t archive.zip                   # list
ozip l file.xz                       # single-file codecs: xz zstd gzip bzip2 lzip lzma-alone
```

### From Ruby

The Ruby gem rides this workspace through prebuilt platform gems — `gem
install omnizip` picks a cdylib for your OS/arch with zero compilation
(`omnizip-ffi` exports a C ABI over the codecs and containers). See the
[omnizip gem](https://github.com/omnizip/omnizip).

### From Rust

Codecs implement the `Codec` trait from `omnizip-codecs` and register on a
`CodecRegistry`; dispatch never branches per codec:

```rust
use omnizip_codecs::{Codec, CodecRegistry, CompressionLevel};

let registry = CodecRegistry::new(); // codecs self-register
let zstd = registry.get("zstd")?;
let compressed = zstd.compress(data, CompressionLevel::new(6))?;
assert_eq!(zstd.decompress(&compressed, data.len())?, data);
```

## Crates

| Group | Crates |
|---|---|
| Codecs | `omnizip-lzma` (LZMA/LZMA2/XZ) · `omnizip-zstd` · `omnizip-brotli` · `omnizip-deflate` · `omnizip-deflate64` · `omnizip-libdeflate` · `omnizip-bzip2` · `omnizip-ppmd` (PPMd7/8) · `omnizip-lz4` · `omnizip-snappy` · `omnizip-glza` · `omnizip-zpaq` · `omnizip-flac` · `omnizip-fsst` · `omnizip-ricepp` · `omnizip-blosc` |
| Shared | `omnizip-codecs` (trait + registry + streaming/chunked/profiles/checksums) · `omnizip-filters` (BCJ x86/ARM/ARM64/IA64/PPC/SPARC, delta, shuffle) · `omnizip-checksum` · `omnizip-crypto` |
| Containers | `omnizip-tar` · `omnizip-zip` (incl. AES, zip64) · `omnizip-sevenzip` · `omnizip-rar` (RAR3/4/5 read, RAR4 LZ/PPMd/AES, RAR5 LZ/AES) · `omnizip-cpio` · `omnizip-iso` · `omnizip-xar` · `omnizip-rpm` · `omnizip-ole` · `omnizip-par2` · `omnizip-archive-core` |
| App / FFI | `ozip` (CLI) · `omnizip-ffi` (C-ABI cdylib for the Ruby gem) · `omnizip-bench` |
| Test crates | `tests/differential` · `tests/determinism` · `tests/property` · `tests/benchmarks` · `tests/conformance` · `tests/security` · `tests/fuzz_smoke` |

Bit-level format specifications live in [`docs/specs/`](docs/specs/);
architecture notes in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Conformance

A divergence between Rust and the Ruby reference — or between Rust and the
`xz`/`zstd`/`7zz` CLI oracles — is a release blocker. CI runs:

- **Differential** (`tests/differential`): every codec against the Ruby
  reference on shared fixtures, both decode and encode directions.
- **Determinism** (`tests/determinism`): archive creation byte-stability.
- **Property + fuzz**: malformed-input safety across all decoders
  (including the extraction-security corpus in `tests/security`).
- **Downstream**: the LimniFS consumer's suite runs against every PR.
- **Performance**: the codec sweep board gates regressions per codec/level.

## License

MIT OR Apache-2.0. The Ruby ports inherit Ribose Inc.'s MIT headers; see
[`LICENSE-NOTICE.md`](LICENSE-NOTICE.md) for attribution.
