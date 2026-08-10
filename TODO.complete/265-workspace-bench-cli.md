# 265 — Workspace Bench CLI

- **Priority:** P3 (DX: unified `omnizip-bench` CLI)
- **Crate:** `omnizip-bench`
- **Depends on:** [247](247-real-world-test-corpora.md)
- **Estimated effort:** 1 day

## Problem

Today, running a benchmark means:

```bash
cargo run -p omnizip-brotli --example brotli_benchmark --release
cargo run -p omnizip-zstd --example zstd_benchmark --release
# ... 13 more codec-specific commands
```

Each codec has its own benchmark binary with its own output format.
Comparing across codecs requires copy-pasting numbers into a
spreadsheet.

## Design

### Unified CLI

```bash
# Run all codecs on a single file
omnizip-bench --input corpus/silesia/dickens

# Run one codec across all corpora files
omnizip-bench --codec brotli --corpus silesia

# Run all codecs on all corpora
omnizip-bench --corpus silesia,enwik8

# JSON output for tools
omnizip-bench --json --codec brotli,zstd,lzma --input my-file > results.json

# Compare two runs
omnizip-bench diff baseline.json current.json
```

### Output format

```
Codec       Level  Input          Original   Compressed  Ratio  Time    MB/s
brotli      5      dickens        10,192,446 3,456,789   33.9%  1.2s    8.2
zstd        9      dickens        10,192,446 3,234,567   31.7%  0.4s   24.1
lzma        6      dickens        10,192,446 3,123,456   30.6%  2.1s    4.7
```

### JSON schema

```json
{
  "version": "0.16.25",
  "results": [
    {
      "codec": "brotli",
      "level": 5,
      "input": "silesia/dickens",
      "original_bytes": 10192446,
      "compressed_bytes": 3456789,
      "ratio": 0.339,
      "elapsed_ms": 1247,
      "mbps": 8.2
    }
  ]
}
```

## Acceptance criteria

- [ ] `omnizip-bench` CLI with `--codec`, `--input`, `--corpus`,
      `--json`, `diff` flags.
- [ ] Tabular default output; JSON via flag.
- [ ] Diff command compares two runs, highlights regressions.
- [ ] README documents all flags.

## Why this matters

A unified CLI makes benchmarks reproducible and shareable. Today
the answer to "how does codec X compare to codec Y?" requires
running two different commands and reading different output
formats. The unified CLI gives one command + one output.
