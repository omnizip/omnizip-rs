# omnizip-bench

Benchmark suite for omnizip-rs codecs. Runs every codec against
standard compression corpora and reports ratio, throughput, and
determinism.

## Quick start

```bash
# Smoke test (no network, synthetic 4 KB inputs)
cargo run -p omnizip-bench --release -- --synthetic 4096

# Real corpus (downloads Calgary on first run, ~3 MB, cached thereafter)
cargo run -p omnizip-bench --release -- --corpus calgary

# Pick codecs and levels
cargo run -p omnizip-bench --release -- --codec zstd,lzma,deflate --level 3,6,9 --corpus calgary

# JSON output for piping into a notebook
cargo run -p omnizip-bench --release -- --corpus calgary --format json > results.json
```

## Layout

MECE modules, dependencies always downward:

| Module      | Responsibility                                          |
|-------------|---------------------------------------------------------|
| `case`      | `BenchCodec`, `BenchmarkResult` — pure data             |
| `corpus`    | Corpus model, downloader, cache, `known_corpora()`      |
| `synthetic` | In-process corpora (zeros/random/text/mixed)            |
| `runner`    | Orchestrates `(codec, level, file)` → `BenchmarkResult` |
| `report`    | `Reporter` trait + CSV / JSON / Markdown impls          |

## Adding a codec

One entry in `default_codecs()` in `src/lib.rs`:

```rust
BenchCodec::new("mycodec", Box::new(MyCodec), vec![1, 6, 9]),
```

No runner or reporter edits — the runner is codec-agnostic (OCP).

## Adding a corpus

One entry in `known_corpora()` in `src/corpus.rs`:

```rust
CorpusSpec {
    name: "mycorpus",
    description: "...",
    approx_size: 1_000_000,
    url: "https://example.com/mycorpus.zip",
    files: &["file1", "file2"],
},
```

## Adding a reporter

Implement the `Reporter` trait:

```rust
pub struct HtmlReporter;
impl Reporter for HtmlReporter {
    fn report(&self, results: &[BenchmarkResult]) -> String { /* ... */ }
}
```

Wire it into the CLI's `format` enum in `src/main.rs`.

## Cache location

`~/.cache/omnizip-bench/<corpus>/` (override with `OMNIZIP_BENCH_CACHE`).

## Determinism

Each case compresses the input **twice**; results are flagged
`deterministic=false` if the two compressed outputs differ. This
directly enforces omnizip-rs's content-addressed-storage invariant.
