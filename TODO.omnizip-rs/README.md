# omnizip-rs — implementation roadmap

All remaining work for the omnizip-rs Rust workspace, decomposed MECE across
codecs, infrastructure, and integration. Each file is self-contained: goals,
dependencies, acceptance criteria, and the Ruby → Rust module map (when
applicable).

## Source of truth

The Ruby implementations in [`omnizip/omnizip`](https://github.com/omnizip/omnizip)
are the **algorithmic reference** for every port. Every Rust module is a
line-by-line translation of the corresponding Ruby file. C references are
perf-tuning consultants, never the porting basis.

## Algorithm portfolio

### Tier A — core codecs (port from omnizip Ruby)

| Codec | Id | Ruby LOC | Status | Priority |
|---|---|---:|---|---|
| LZMA/LZMA2/XZ | TBD | 8,464 | porting | P0 |
| ZSTD | TBD | 3,150 | porting | P0 |
| DEFLATE | TBD | 110 | porting | P1 |
| DEFLATE64 | TBD | 783 | porting | P1 |
| bzip2 | TBD | 1,101 | porting | P1 |
| PPMd7 | TBD | 807 | porting | P2 |
| PPMd8 | TBD | 656 | porting | P2 |

### Tier B — filters (port from omnizip Ruby)

| Filter | Ruby LOC | Priority |
|---|---:|---|
| BCJ x86 / ARM / ARM64 / IA64 / PPC / SPARC | ~600 | P1 |
| BCJ2 (x86 4-stream) | ~400 | P2 |
| Delta | ~100 | P1 |
| XZ Delta | ~100 | P1 |

### Tier C — newer algorithms (research, then port or wrap)

Researched in [26-newer-algorithm-watch.md](26-newer-algorithm-watch.md).

| Algorithm | Origin | Pure Rust? | Priority |
|---|---|---|---|
| Snappy | Google 2011 | `snap` crate | P2 |
| libdeflate | Eric Biggers 2016 | port needed | P2 |
| LZ4 HC | Collet 2011 (HC variant) | `lz4_flex` supports it | P2 |
| ZSTD dictionaries | Facebook 2018+ | port with ZSTD | P2 |
| LZO | Oberhumer 1996 | port needed | P3 |
| FastLZ | Ariya Hidayat 2008 | port needed | P3 |
| Density v2 | alfa-1 2017 | pure Rust exists | P3 |
| ZPAQ | Matt Mahoney 2009 | port needed (complex) | P3 |
| GLZA | Gregory Jackson 2017 | port needed (research) | P3 |

### Tier D — hardware / learned (research only, 2026+)

Not in scope for pure-Rust ports. Tracked in [26-newer-algorithm-watch.md](26-newer-algorithm-watch.md).

| Algorithm | Status | Notes |
|---|---|---|
| Intel IAA (hardware) | watch only | Sapphire Rapids+; not a software target |
| ARM SVE compression | watch only | AArch64 extension; not portable |
| Learned compression (ML) | rejected | non-deterministic; violates air-gapped build rule |

## Cross-cutting concerns

| Concern | Owner | Priority |
|---|---|---|
| Codec trait + registry | [01](01-codec-trait-registry.md) | P0 |
| Cross-language differential harness | [02](02-cross-language-differential-harness.md) | P0 |
| Conformance corpus | [03](03-conformance-corpus.md) | P0 |
| Benchmark suite | [30](30-benchmark-suite.md) | P1 |
| Fuzz targets | [31](31-fuzz-targets.md) | P1 |
| SIMD acceleration | [32](32-simd-acceleration.md) | P2 |
| Multi-threaded encoding | [33](33-multi-threaded-encoding.md) | P2 |
| no_std / embedded | [34](34-no-std-support.md) | P3 |
| crates.io publishing | [35](35-crates-io-publishing.md) | P2 |

## Priority order

| # | File | Tier | Priority | Depends on |
|---|---|---|---|---|
| 00 | [00-architecture.md](00-architecture.md) | Foundation | P0 | — |
| 01 | [01-codec-trait-registry.md](01-codec-trait-registry.md) | Foundation | P0 | 00 |
| 02 | [02-cross-language-differential-harness.md](02-cross-language-differential-harness.md) | Foundation | P0 | 00 |
| 03 | [03-conformance-corpus.md](03-conformance-corpus.md) | Foundation | P0 | — |
| 10 | [10-lzma-phase-a-decoder.md](10-lzma-phase-a-decoder.md) | LZMA | P0 | 01, 02 |
| 11 | [11-lzma-phase-b-encoder.md](11-lzma-phase-b-encoder.md) | LZMA | P1 | 10 |
| 12 | [12-lzma-phase-c-optimal-xz.md](12-lzma-phase-c-optimal-xz.md) | LZMA | P1 | 11 |
| 13 | [13-zstd-phase-a-decoder.md](13-zstd-phase-a-decoder.md) | ZSTD | P0 | 01, 02 |
| 14 | [14-zstd-phase-b-encoder.md](14-zstd-phase-b-encoder.md) | ZSTD | P1 | 13 |
| 15 | [15-zstd-phase-c-fse.md](15-zstd-phase-c-fse.md) | ZSTD | P1 | 14 |
| 16 | [16-deflate.md](16-deflate.md) | DEFLATE | P1 | 01, 02 |
| 17 | [17-bzip2.md](17-bzip2.md) | bzip2 | P1 | 01, 02 |
| 18 | [18-ppmd.md](18-ppmd.md) | PPMd | P2 | 01, 02 |
| 19 | [19-bcj-filters.md](19-bcj-filters.md) | Filters | P1 | 01 |
| 20 | [20-snappy.md](20-snappy.md) | Newer | P2 | 01 |
| 21 | [21-libdeflate.md](21-libdeflate.md) | Newer | P2 | 16 |
| 22 | [22-lz4-hc.md](22-lz4-hc.md) | Newer | P2 | 01 |
| 23 | [23-zstd-dictionaries.md](23-zstd-dictionaries.md) | Newer | P2 | 14 |
| 24 | [24-zpaq.md](24-zpaq.md) | Research | P3 | 01 |
| 25 | [25-glza.md](25-glza.md) | Research | P3 | 01 |
| 26 | [26-newer-algorithm-watch.md](26-newer-algorithm-watch.md) | Research | P3 | — |
| 30 | [30-benchmark-suite.md](30-benchmark-suite.md) | Infra | P1 | 03 |
| 31 | [31-fuzz-targets.md](31-fuzz-targets.md) | Infra | P1 | 03 |
| 32 | [32-simd-acceleration.md](32-simd-acceleration.md) | Infra | P2 | 10, 13 |
| 33 | [33-multi-threaded-encoding.md](33-multi-threaded-encoding.md) | Infra | P2 | 11, 14 |
| 34 | [34-no-std-support.md](34-no-std-support.md) | Infra | P3 | 10 |
| 35 | [35-crates-io-publishing.md](35-crates-io-publishing.md) | Infra | P2 | 10 |
| 40 | [40-limnifs-integration.md](40-limnifs-integration.md) | Integration | P1 | 10, 13 |

## Execution rules

1. **One `in_progress` task per crate at a time.** Move tasks through `pending`
   → `in_progress` → `done` by editing the task file header.
2. **No shims, no stubs.** A task is `done` only when its acceptance criteria
   pass in CI (linux + macOS, stable Rust, with the cross-language differential
   harness green).
3. **Ruby is the algorithmic oracle.** Any divergence between Rust output and
   Ruby output on the same fixture is a release blocker.
4. **Spec-first.** Wire-format and codec-id changes update
   `omnizip-rs/PLAN.md` + this README before code.
5. **Rebase-merge all PRs.** No direct pushes to `main` except the initial
   skeleton commit per repo.
