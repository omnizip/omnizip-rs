# omnizip-rs — Pure-Rust compression codecs

Pure-Rust implementations of LZMA, ZSTD, Brotli, DEFLATE, bzip2, and PPMd,
ported from the [omnizip](https://github.com/omnizip/omnizip) Ruby reference
implementations. MIT OR Apache-2.0.

## Why this repo exists

[omnizip](https://github.com/omnizip/omnizip) ships pure-Ruby implementations
of the major compression codecs. Ruby is too slow for production codec use,
but the **algorithms are correct and tested**. This repo ports them to Rust
for production-grade speed, keeping the Ruby as the authoritative reference.

Every Rust module is a line-by-line translation of the corresponding Ruby
file. The file-level mapping lives in [`PLAN.md`](PLAN.md).

## Cross-language verification

The Rust crates' test suites run the same fixtures as the Ruby specs and
assert byte-identical output. CI clones the omnizip Ruby repo and runs both
implementations against the `.xz` / `.zst` / `.lzma` vectors under
`omnizip/spec/fixtures/`. A divergence between Ruby and Rust is a release
blocker.

## Crates

| Crate | Status | Ruby reference | C reference (perf tuning only) |
|---|---|---|---|
| `omnizip-lzma` | porting | `omnizip/lib/omnizip/algorithms/lzma/` (7,558 LOC) | `tukaani-project/xz` liblzma (0BSD) |
| `omnizip-zstd` | porting | `omnizip/lib/omnizip/algorithms/zstandard/` (3,150 LOC) | `facebook/zstd` (BSD-3-Clause) |
| `omnizip-brotli` | planned | — | `brotli` crate (already pure Rust) |
| `omnizip-deflate` | planned | `omnizip/lib/omnizip/algorithms/deflate/` | `miniz_oxide` (already pure Rust) |
| `omnizip-bzip2` | planned | `omnizip/lib/omnizip/algorithms/bzip2/` | — |
| `omnizip-ppmd` | planned | `omnizip/lib/omnizip/algorithms/ppmd7/`, `ppmd8/` | — |

## License

MIT OR Apache-2.0, matching the per-file headers in omnizip's Ruby source.
The Ruby code is MIT-licensed by Ribose Inc.; this Rust port inherits that
license. See [`LICENSE-NOTICE.md`](LICENSE-NOTICE.md) for full attribution.

## Consumers

- [LimniFS](https://github.com/limnifs/limnifs) — content-addressed filesystem
  image format; consumes `omnizip-lzma` and `omnizip-zstd` as codec plugins
  via the `Codec` trait registry.

## Status

**Phase A (decode) ships for LZMA and ZSTD.** Both crates decode their
respective formats against the reference `xz -d` / `zstd -d` oracles on
every fixture under `tests/fixtures/`. Encoders and optimal parsers
(Phases B/C) per [`PLAN.md`](PLAN.md).
