# 86 — Benchmark suite (Silesia + Enwik8 + Calgary)

**Priority:** High — ✅ **DONE**
**Source:** RESEARCH.md §9 (no benchmarks today)

## Status

`omnizip-bench` crate landed as a workspace member. Runs every
registered codec against standard corpora, produces ratio +
encode/decode throughput numbers, and verifies determinism + round-
trip correctness on every case.

## Architecture

Three MECE layers, dependencies always downward:

1. **`case`** — `BenchCodec`, `BenchmarkResult`, `CodecError` (pure data)
2. **`corpus`** — `Corpus`, `CorpusFile`, downloader, `known_corpora()`
3. **`synthetic`** — in-process corpora (zeros/random/text/mixed) for CI
4. **`runner`** — orchestrates `(codec, level, file)` → `BenchmarkResult`
5. **`report`** — `Reporter` trait with CSV / JSON / Markdown impls

Open/closed: adding a codec = one entry in `default_codecs()`. Adding
a corpus = one entry in `known_corpora()`. Adding a reporter = one
`impl Reporter`. The runner never changes.

## Corpora wired

- `calgary` / `canterbury` — download from corpus.canterbury.ac.nz
- `silesia` — download from sun.aei.polsl.pl
- `enwik8` — download from mattmahoney.net
- `ait2026` — placeholder URL (real URL pending; see TODO 90)
- Synthetic: `zeros`, `random`, `text`, `mixed` (no network)

## Usage

```bash
cargo run -p omnizip-bench --release -- --synthetic 4096
cargo run -p omnizip-bench --release -- --corpus calgary
cargo run -p omnizip-bench --release -- --codec zstd,lzma --level 3,6,9 --corpus calgary --format json
```

## Tests

14 unit tests pass: corpus model, runner orchestration, reporter
formats, synthetic corpora, edge cases.

## Context

omnizip-rs has 835+ tests but **zero benchmarks**. We can't claim
competitiveness without numbers. The standard corpora every
compression paper uses:

| Corpus   | Size     | Content                              |
|----------|----------|--------------------------------------|
| Silesia  | ~200 MB  | Web, source, binary, text mix       |
| Enwik8   | 100 MB   | Wikipedia XML (English)              |
| Calgary  | ~3 MB    | Classic small corpus (text+binary)   |
| Canterbury | ~3 MB  | Updated Calgary                     |

Plus our own synthetic fixtures: pseudo-random, all-zero, repeated
patterns, mixed binary/text.

## Design

New `omnizip-bench/` crate at workspace root. CLI tool:

```
$ omnizip-bench --codec zstd,brotli,lzma,ppmd7 --level 3,6,9 --corpus silesia
codec     level  input_size  compressed  ratio   enc_ms  dec_ms
zstd      3      211,998,188  78,345,231  0.369   4,231   1,124
zstd      6      211,998,188  65,234,109  0.308   8,452   1,234
...
```

Output: CSV to stdout, human-readable table to stderr.

## Architecture

```rust
trait BenchmarkCodec {
    fn name(&self) -> &'static str;
    fn compress(&self, input: &[u8], level: u8) -> Vec<u8>;
    fn decompress(&self, compressed: &[u8], expected_len: usize) -> Vec<u8>;
}
```

Each codec crate ships a `BenchmarkCodec` impl. `omnizip-bench`
collects them and runs each on each corpus file.

For corpora:
- Don't bundle large files (they bloat the git repo and the published
  crate). Instead, download on demand from canonical URLs.
- Cache in `~/.cache/omnizip-bench/`.

## Acceptance criteria

- [ ] `omnizip-bench` crate created as workspace member.
- [ ] CLI accepts `--codec`, `--level`, `--corpus`, `--iterations`.
- [ ] All 17 codecs have `BenchmarkCodec` impls.
- [ ] Output CSV with: codec, level, input_size, compressed_size,
      ratio, enc_time_ms, dec_time_ms.
- [ ] Documentation: README in `omnizip-bench/` explaining how to
      add new corpora and codecs.
- [ ] Sample run produces publishable numbers (we'll post in the
      omnizip-rs README comparison table).

## Files

- `omnizip-bench/Cargo.toml`
- `omnizip-bench/src/main.rs` — CLI entry
- `omnizip-bench/src/bench.rs` — benchmark runner
- `omnizip-bench/src/corpora.rs` — corpus download/cache
- `omnizip-bench/src/codecs/` — one impl per codec
- `omnizip-bench/README.md` — usage docs
