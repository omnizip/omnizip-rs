# 247 — Real-World Test Corpora Acquisition

- **Status:** PARTIAL — `tests/fixtures/corpora/setup.sh` downloads
  Silesia, Enwik8, Calgary, Canterbury on demand. Corpora are
  gitignored. LimniFS csv-synthetic still pending user-provided copy.
- **Priority:** P1 (blocks meaningful ratio claims)
- **Crate:** workspace (`tests/fixtures/corpora/`)
- **Depends on:** none
- **Estimated effort:** 1-2 days

## Problem

Current ratio claims are based on synthetic inputs generated in
`brotli_benchmark.rs` (csv_data, english_text, binary_data,
mixed_data). These don't represent real-world data:

- Synthetic CSV uses uniform value patterns (no realistic skew).
- Synthetic "English text" is one phrase repeated.
- Binary data is a single LCG output (not representative of object
  files, images, audio).

This means we cannot:
- Verify ratio claims against published Brotli/ZSTD benchmarks.
- Detect regressions on data shapes that matter to users.
- Compare meaningfully against the vendored C reference (which the
  user reports achieves 3.6% on their real csv-synthetic).

## Standard corpora to acquire

### Silesia (≈ 200 MB, de-facto compression benchmark)
- Sources: `https://sun.aei.polsl.pl/~sdeor/index.php?page=silesia`
- Contents: dickens, mozilla, mr, nci, ooffice, osdb, reymont,
  samba, sao, webster, x-ray, xml
- License: research/educational use

### Enwik8 (100 MB, Wikipedia XML)
- Source: `https://mattmahoney.net/dc/textdata.html`
- Used by Hutter Prize & most compression papers
- License: GFDL/GPL

### Calgary Corpus (3 MB, classic)
- Source: `https://www.data-compression.info/Corpora/CalgaryCorpus/`
- 14 files: text, code, object, image data
- Public domain

### Canterbury Corpus (3 MB, modernized Calgary)
- Source: `https://corpus.canterbury.ac.nz/`
- 18 files, designed to address Calgary's datedness
- Public domain

### LimniFS csv-synthetic (20 MB, user's workload)
- Source: LimniFS internal — request from user
- Used in user's reported benchmarks
- Critical: this is the data the user cares about

## Design

### Storage layout

```
tests/fixtures/corpora/
├── README.md                          # licensing, sources, notes
├── silesia/                           # individual files
│   ├── dickens
│   ├── mozilla
│   └── ...
├── enwik8                             # single file
├── calgary/
│   ├── bib
│   ├── book1
│   └── ...
├── canterbury/
│   └── ...
└── limnifs/                           # added by user
    └── csv-synthetic
```

Corpora are gitignored by default (too large for the repo). The
`tests/corpora/setup.sh` script downloads them on demand. CI runs
the corpora benchmarks only when `OMNIZIP_RUN_CORPORA=1` is set.

### Benchmark runner

Extend `omnizip-bench` with `--corpus <name>`:

```bash
# Per-file ratio + speed
cargo run -p omnizip-bench --release -- --corpus silesia

# Aggregate stats (geomean ratio, mean speed)
cargo run -p omnizip-bench --release -- --corpus silesia --summary
```

Output format mirrors the Silesia benchmark convention:

```
File            Original    Brotli-Q5   ZSTD-L9    LZMA-9
dickens         10,192,446  3,456,789   3,234,567  3,123,456
mozilla         51,220,160  18,234,567  17,123,456 ...
...
```

### Regression baseline

Once corpora are in place, run benchmarks at every merge to main.
Store results in `tests/corpora/baseline.json`. CI fails if the
geomean ratio regresses by > 1% OR speed regresses by > 5%.

## Acceptance criteria

- [ ] All four standard corpora downloaded and committed to LFS
      (or scripted via setup.sh).
- [ ] `omnizip-bench --corpus silesia` runs end-to-end.
- [ ] `tests/corpora/baseline.json` committed with current numbers.
- [ ] GHA workflow runs the corpora benchmark on PRs touching
      encoder code.
- [ ] LimniFS csv-synthetic added (user-provided).

## Why this matters

Without real-world test data, "ratio improved by 10%" is a claim
about synthetic data that may not generalize. Real corpora let us:
- Detect regressions on data shapes the user actually has.
- Make publishable claims (Silesia is the de-facto standard).
- Compare against published Brotli/ZSTD/LZMA numbers.
